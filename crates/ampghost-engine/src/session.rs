use std::sync::Arc;
use std::time::{Duration, Instant};

use ampghost_midi::MidiTransport;
use ampghost_protocol::{
    Channel, DeviceProfile, Dt1, IDENTITY_REQUEST, Identity, Incoming, PatchAssembler, PatchImage,
    PatchOffset, ProgramChangeMap, ProtocolError, Rq1, parse_current_channel, profile_for,
    program_change, verify,
};

use crate::clock::Clock;
use crate::command::{AmpSwitch, DeviceInfo, Event, Stage, SwitchOutcome, WriteOutcome};
use crate::error::EngineError;
use crate::timings::Timings;

pub(crate) struct Session {
    transport: Box<dyn MidiTransport>,
    clock: Arc<dyn Clock>,
    timings: Timings,
    device: DeviceInfo,
    unsolicited: Vec<Dt1>,
    program_change_ready: Option<Instant>,
    editor_mode: bool,
}

impl Session {
    pub(crate) fn identify(
        mut transport: Box<dyn MidiTransport>,
        clock: Arc<dyn Clock>,
        timings: Timings,
    ) -> Result<Self, EngineError> {
        let port = transport.port_name().to_owned();
        transport.send(&IDENTITY_REQUEST)?;
        let deadline = clock.now() + timings.identity_timeout;
        loop {
            let remaining = deadline.saturating_duration_since(clock.now());
            if remaining.is_zero() {
                return Err(EngineError::IdentityTimeout { port });
            }
            let Some(bytes) = transport.receive(remaining)? else {
                continue;
            };
            if let Ok(identity) = Identity::parse(&bytes) {
                let profile = profile_for(&identity);
                tracing::info!(%port, %identity, supported = profile.is_some(), "identity reply");
                return Ok(Self {
                    transport,
                    clock,
                    timings,
                    device: DeviceInfo {
                        port,
                        identity,
                        profile,
                    },
                    unsolicited: Vec::new(),
                    program_change_ready: None,
                    editor_mode: false,
                });
            }
        }
    }

    pub(crate) fn is_present(&self) -> bool {
        self.transport.is_present()
    }

    pub(crate) fn device(&self) -> &DeviceInfo {
        &self.device
    }

    pub(crate) fn profile(&self) -> Result<&'static DeviceProfile, EngineError> {
        self.device.profile.ok_or_else(|| EngineError::Unsupported {
            identity: self.device.identity.hex(),
        })
    }

    fn decode(&self, bytes: &[u8]) -> Result<Incoming, ProtocolError> {
        match self.device.profile {
            Some(profile) => profile.decode(bytes),
            None => Ok(Identity::parse(bytes)
                .map_or_else(|_| Incoming::Other(bytes.to_vec()), Incoming::Identity)),
        }
    }

    fn send_request(&mut self, request: Rq1) -> Result<(), EngineError> {
        let bytes = self.profile()?.encode_request(&request);
        self.transport.send(&bytes)?;
        Ok(())
    }

    pub(crate) fn take_unsolicited(&mut self) -> Vec<Dt1> {
        std::mem::take(&mut self.unsolicited)
    }

    fn next_message(&mut self, deadline: Instant) -> Result<Option<Incoming>, EngineError> {
        let remaining = deadline.saturating_duration_since(self.clock.now());
        if remaining.is_zero() {
            return Ok(None);
        }
        let Some(bytes) = self.transport.receive(remaining)? else {
            return Ok(None);
        };
        match self.decode(&bytes) {
            Ok(message) => Ok(Some(message)),
            Err(error) => {
                tracing::warn!(
                    error = &error as &dyn std::error::Error,
                    "dropping invalid message"
                );
                Ok(Some(Incoming::Other(Vec::new())))
            }
        }
    }

    pub(crate) fn drain(&mut self, duration: Duration) -> Result<(), EngineError> {
        let deadline = self.clock.now() + duration;
        while let Some(message) = self.next_message(deadline)? {
            if let Incoming::Dt1(packet) = message {
                tracing::debug!(%packet, "unsolicited");
                self.unsolicited.push(packet);
            }
        }
        Ok(())
    }

    fn request(&mut self, request: Rq1) -> Result<Vec<Dt1>, EngineError> {
        self.drain(self.timings.pre_request_drain)?;
        self.send_request(request)?;
        let start = request.address().linear();
        let end = request.end();
        let deadline = self.clock.now() + self.timings.read_timeout;
        let mut packets = Vec::new();
        loop {
            match self.next_message(deadline)? {
                None => {
                    return Err(EngineError::Timeout {
                        address: request.address(),
                        received: packets.len(),
                    });
                }
                Some(Incoming::Dt1(packet)) if packet.start() >= start && packet.end() <= end => {
                    let finished = packet.end() == end;
                    packets.push(packet);
                    if finished {
                        return Ok(packets);
                    }
                }
                Some(Incoming::Dt1(packet)) => self.unsolicited.push(packet),
                Some(_) => {}
            }
        }
    }

    fn read_bytes(&mut self, request: Rq1) -> Result<Vec<u8>, EngineError> {
        let packets = self.request(request)?;
        let start = request.address().linear();
        let len = usize::try_from(request.size().linear()).unwrap_or_default();
        let mut bytes = vec![None; len];
        for packet in &packets {
            let first = usize::try_from(packet.start() - start).unwrap_or_default();
            for (slot, &value) in bytes.iter_mut().skip(first).zip(packet.data()) {
                *slot = Some(value);
            }
        }
        bytes
            .into_iter()
            .collect::<Option<Vec<u8>>>()
            .ok_or(EngineError::Timeout {
                address: request.address(),
                received: packets.len(),
            })
    }

    pub(crate) fn read_exact(&mut self, request: Rq1) -> Result<Vec<u8>, EngineError> {
        self.drain(self.timings.backup_gap)?;
        tracing::debug!(address = %request.address(), size = request.size().linear(), "backup RQ1");
        self.send_request(request)?;
        let start = request.address().linear();
        let end = request.end();
        let expected = usize::try_from(request.size().linear()).unwrap_or_default();
        let deadline = self.clock.now() + self.timings.backup_timeout;
        loop {
            let remaining = deadline.saturating_duration_since(self.clock.now());
            if remaining.is_zero() {
                return Err(EngineError::Timeout {
                    address: request.address(),
                    received: 0,
                });
            }
            let Some(bytes) = self.transport.receive(remaining)? else {
                continue;
            };
            let packet = match self.decode(&bytes) {
                Ok(Incoming::Dt1(packet)) => packet,
                Ok(_) => continue,
                Err(error @ ProtocolError::Checksum { address, .. })
                    if address.linear() >= start && address.linear() < end =>
                {
                    return Err(error.into());
                }
                Err(error) => {
                    tracing::warn!(
                        error = &error as &dyn std::error::Error,
                        "dropping invalid message during backup"
                    );
                    continue;
                }
            };
            let overlaps = packet.start() < end && packet.end() > start;
            if packet.address() == request.address() && packet.data().len() == expected {
                return Ok(packet.data().to_vec());
            }
            if overlaps {
                return Err(EngineError::BackupReply {
                    requested: request.address(),
                    expected,
                    replied: packet.address(),
                    actual: packet.data().len(),
                });
            }
            self.unsolicited.push(packet);
        }
    }

    pub(crate) fn read_temp(&mut self) -> Result<PatchImage, EngineError> {
        let started = self.clock.now();
        let attempts = self.timings.read_attempts.max(1);
        let mut attempt = 1;
        let result = loop {
            match self.read_temp_once() {
                Err(error) if attempt < attempts && read_can_retry(&error) => {
                    tracing::warn!(
                        error = &error as &dyn std::error::Error,
                        attempt,
                        "temp read incomplete, reading again"
                    );
                    attempt += 1;
                }
                other => break other,
            }
        };
        self.hold_off_program_change();
        let image = result?;
        tracing::debug!(
            attempt,
            millis = self.clock.now().duration_since(started).as_millis(),
            "temp read"
        );
        Ok(image)
    }

    fn read_temp_once(&mut self) -> Result<PatchImage, EngineError> {
        let profile = self.profile()?;
        let request = profile.read_temp_request();
        self.drain(self.timings.pre_request_drain)?;
        self.discard_superseded(request);
        self.send_request(request)?;
        let deadline = self.clock.now() + self.timings.read_timeout;
        let mut assembler = PatchAssembler::new(&profile.patch_layout, profile.temp_patch);
        let mut received = 0;
        loop {
            match self.next_message(deadline)? {
                None if received == 0 => {
                    return Err(EngineError::Timeout {
                        address: request.address(),
                        received,
                    });
                }
                None => return Ok(assembler.finish()?),
                Some(Incoming::Dt1(packet)) if assembler.add(&packet) => {
                    received += 1;
                    if assembler.is_complete() {
                        return Ok(assembler.finish()?);
                    }
                }
                Some(Incoming::Dt1(packet)) => self.unsolicited.push(packet),
                Some(_) => {}
            }
        }
    }

    fn discard_superseded(&mut self, request: Rq1) {
        let start = request.address().linear();
        let end = request.end();
        let before = self.unsolicited.len();
        self.unsolicited
            .retain(|packet| packet.end() <= start || packet.start() >= end);
        let discarded = before - self.unsolicited.len();
        if discarded > 0 {
            tracing::info!(
                discarded,
                "dropping temp packets superseded by a fresh read"
            );
        }
    }

    pub(crate) fn follow_amp_switch(
        &mut self,
        packets: &[Dt1],
    ) -> Result<Option<AmpSwitch>, EngineError> {
        let profile = self.profile()?;
        let Some(position) = packets
            .iter()
            .rposition(|packet| packet.address() == profile.current_channel)
        else {
            return Ok(None);
        };
        let mut channel = packets
            .get(position)
            .map(|packet| parse_current_channel(packet.data()))
            .transpose()?;
        tracing::info!(
            ?channel,
            "amp switched channel by itself, collecting its dump"
        );
        self.hold_off_program_change();
        let mut assembler = PatchAssembler::new(&profile.patch_layout, profile.temp_patch);
        for packet in packets.iter().skip(position + 1) {
            assembler.add(packet);
        }
        let limit = self.clock.now() + self.timings.dump_limit;
        while !assembler.is_complete() {
            let quiet = (self.clock.now() + self.timings.dump_quiet).min(limit);
            match self.next_message(quiet)? {
                None => break,
                Some(Incoming::Dt1(packet)) => {
                    if packet.address() == profile.current_channel {
                        channel = Some(parse_current_channel(packet.data())?);
                        assembler = PatchAssembler::new(&profile.patch_layout, profile.temp_patch);
                    } else {
                        assembler.add(&packet);
                    }
                    self.unsolicited.push(packet);
                }
                Some(_) => {}
            }
        }
        let Some(channel) = channel else {
            return Ok(None);
        };
        let missing = assembler.missing();
        if let Ok(temp) = assembler.finish() {
            tracing::info!(%channel, "temp taken from the amp's own dump");
            return Ok(Some(AmpSwitch {
                channel,
                temp,
                from_dump: true,
            }));
        }
        tracing::info!(%channel, missing, "amp dump incomplete, reading temp after hold-off");
        self.wait_for_program_change(&mut |_| {})?;
        let temp = self.read_temp()?;
        Ok(Some(AmpSwitch {
            channel,
            temp,
            from_dump: false,
        }))
    }

    pub(crate) fn read_current_channel(&mut self) -> Result<Channel, EngineError> {
        let profile = self.profile()?;
        let bytes = self.read_bytes(profile.read_current_channel_request())?;
        Ok(parse_current_channel(&bytes)?)
    }

    pub(crate) fn read_program_change_map(&mut self) -> Result<ProgramChangeMap, EngineError> {
        let profile = self.profile()?;
        let bytes = self.read_bytes(profile.read_program_change_map_request())?;
        Ok(ProgramChangeMap::parse(&bytes)?)
    }

    fn send_packets(&mut self, packets: &[Dt1]) -> Result<(), EngineError> {
        let profile = self.profile()?;
        for packet in packets {
            profile.check_write(packet)?;
        }
        for packet in packets {
            tracing::debug!(%packet, "write");
            self.transport.send(&profile.encode_write(packet))?;
            self.drain(self.timings.packet_gap)?;
        }
        self.hold_off_program_change();
        Ok(())
    }

    fn hold_off_program_change(&mut self) {
        self.program_change_ready = Some(self.clock.now() + self.timings.program_change_holdoff);
    }

    fn wait_for_program_change(&mut self, sink: &mut dyn FnMut(Event)) -> Result<(), EngineError> {
        let Some(ready) = self.program_change_ready else {
            return Ok(());
        };
        let wait = ready.saturating_duration_since(self.clock.now());
        if !wait.is_zero() {
            tracing::info!(
                millis = wait.as_millis(),
                "waiting for amp to accept program change"
            );
            let millis = u64::try_from(wait.as_millis()).unwrap_or(u64::MAX);
            sink(Event::Progress(Stage::WaitingForAmp { millis }));
            self.drain(wait)?;
        }
        Ok(())
    }

    fn editor_mode_on(&mut self, sink: &mut dyn FnMut(Event)) -> Result<(), EngineError> {
        let profile = self.profile()?;
        sink(Event::Progress(Stage::EditorMode));
        self.send_packets(&[profile.editor_mode_on()?])?;
        self.editor_mode = true;
        Ok(())
    }

    pub(crate) fn leave_editor_mode(&mut self) -> Result<bool, EngineError> {
        if !self.editor_mode {
            return Ok(false);
        }
        let profile = self.profile()?;
        let packet = profile.editor_mode_off()?;
        profile.check_write(&packet)?;
        tracing::debug!(%packet, "write");
        self.transport.send(&profile.encode_write(&packet))?;
        self.editor_mode = false;
        Ok(true)
    }

    pub(crate) fn write_verified(
        &mut self,
        target: &PatchImage,
        offsets: Option<&[PatchOffset]>,
        sink: &mut dyn FnMut(Event),
    ) -> Result<WriteOutcome, EngineError> {
        let profile = self.profile()?;
        let conformed;
        let target = if offsets.is_some() {
            target
        } else {
            let mut image = target.clone();
            for change in ampghost_params::conform_to_official(&mut image)? {
                tracing::info!(
                    offset = %change.offset,
                    from = change.from,
                    to = change.to,
                    "normalised a fixed parameter to the official value"
                );
            }
            conformed = image;
            &conformed
        };
        self.editor_mode_on(sink)?;
        let plan = match offsets {
            Some(offsets) => profile.partial_write_plan(target, offsets)?,
            None => profile.full_write_plan(target)?,
        };
        let mut packets = plan.len();
        sink(Event::Progress(Stage::Writing { packets }));
        self.send_packets(&plan)?;
        self.drain(self.timings.settle)?;
        sink(Event::Progress(Stage::Verifying { attempt: 1 }));
        let actual = self.read_temp()?;
        let report = verify(target, &actual, &profile.verify_rules);
        if report.is_ok() {
            return Ok(WriteOutcome {
                attempts: 1,
                packets,
                report,
                image: actual,
            });
        }
        let differing = report
            .mismatches
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ");
        tracing::warn!(
            bytes = report.mismatches.len(),
            %differing,
            "verify mismatch, rewriting differing bytes once"
        );
        let retry = profile.partial_write_plan(target, &report.mismatch_offsets())?;
        packets += retry.len();
        sink(Event::Progress(Stage::Rewriting {
            bytes: report.mismatches.len(),
        }));
        self.send_packets(&retry)?;
        self.drain(self.timings.settle)?;
        sink(Event::Progress(Stage::Verifying { attempt: 2 }));
        let actual = self.read_temp()?;
        let report = verify(target, &actual, &profile.verify_rules);
        if report.is_ok() {
            Ok(WriteOutcome {
                attempts: 2,
                packets,
                report,
                image: actual,
            })
        } else {
            Err(EngineError::VerifyFailed {
                mismatches: report.mismatches,
            })
        }
    }

    pub(crate) fn write_chain(
        &mut self,
        target: &PatchImage,
        sink: &mut dyn FnMut(Event),
    ) -> Result<(usize, PatchImage), EngineError> {
        let profile = self.profile()?;
        let plan = profile.chain_write_plan(target)?;
        self.editor_mode_on(sink)?;
        let packets = plan.len();
        sink(Event::Progress(Stage::Writing { packets }));
        self.send_packets(&plan)?;
        self.drain(self.timings.settle)?;
        sink(Event::Progress(Stage::Verifying { attempt: 1 }));
        let actual = self.read_temp()?;
        Ok((packets, actual))
    }

    pub(crate) fn switch_channel(
        &mut self,
        channel: Channel,
        sink: &mut dyn FnMut(Event),
    ) -> Result<SwitchOutcome, EngineError> {
        let profile = self.profile()?;
        if !profile.has_channel(channel) {
            return Err(EngineError::ChannelUnavailable(channel));
        }
        let map = self.read_program_change_map()?;
        let program = map.program(channel);
        tracing::info!(%channel, program, "program change map");
        let mut actual = None;
        for attempt in 1..=self.timings.channel_attempts {
            self.wait_for_program_change(sink)?;
            sink(Event::Progress(Stage::ProgramChange { program, attempt }));
            self.transport.send(&program_change(program))?;
            self.hold_off_program_change();
            self.drain(self.timings.channel_settle)?;
            sink(Event::Progress(Stage::ConfirmingChannel { attempt }));
            let current = self.read_current_channel()?;
            tracing::info!(%current, attempt, "channel readback");
            actual = Some(current);
            if current == channel {
                self.drain(self.timings.channel_settle)?;
                sink(Event::Progress(Stage::ReadingTemp));
                let temp = self.read_temp()?;
                return Ok(SwitchOutcome {
                    channel,
                    program,
                    attempts: attempt,
                    reloaded: false,
                    temp,
                });
            }
            tracing::warn!(%channel, %current, attempt, "program change not confirmed");
        }
        Err(EngineError::ChannelSelectFailed {
            wanted: channel,
            actual,
        })
    }

    pub(crate) fn bounce_channel(
        &mut self,
        channel: Channel,
        sink: &mut dyn FnMut(Event),
    ) -> Result<SwitchOutcome, EngineError> {
        let profile = self.profile()?;
        let other = profile
            .channels
            .iter()
            .copied()
            .find(|&candidate| candidate != channel)
            .ok_or(EngineError::ChannelUnavailable(channel))?;
        let map = self.read_program_change_map()?;
        let program = map.program(other);
        tracing::info!(%channel, %other, program, "bouncing through another channel");
        sink(Event::Progress(Stage::Bouncing { channel }));
        self.wait_for_program_change(sink)?;
        self.transport.send(&program_change(program))?;
        self.hold_off_program_change();
        self.drain(self.timings.channel_settle)?;
        let mut outcome = self.switch_channel(channel, sink)?;
        outcome.reloaded = true;
        Ok(outcome)
    }
}

fn read_can_retry(error: &EngineError) -> bool {
    matches!(
        error,
        EngineError::Timeout { .. } | EngineError::Protocol(ProtocolError::IncompleteImage { .. })
    )
}
