use std::sync::Arc;
use std::time::{Duration, Instant};

use katana_midi::MidiTransport;
use katana_protocol::{
    Channel, DeviceProfile, Dt1, IDENTITY_REQUEST, Incoming, PatchImage, PatchOffset,
    ProgramChangeMap, Rq1, decode, parse_current_channel, profile_for, program_change, verify,
};

use crate::clock::Clock;
use crate::command::{DeviceInfo, Event, Stage, SwitchOutcome, WriteOutcome};
use crate::error::EngineError;
use crate::timings::Timings;

pub(crate) struct Session {
    transport: Box<dyn MidiTransport>,
    clock: Arc<dyn Clock>,
    timings: Timings,
    device: DeviceInfo,
    unsolicited: Vec<Dt1>,
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
                return Err(EngineError::NotFound {
                    last: Some(format!("no identity reply on {port}")),
                });
            }
            let Some(bytes) = transport.receive(remaining)? else {
                continue;
            };
            if let Ok(Incoming::Identity(identity)) = decode(&bytes) {
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
                });
            }
        }
    }

    pub(crate) fn device(&self) -> &DeviceInfo {
        &self.device
    }

    pub(crate) fn profile(&self) -> Result<&'static DeviceProfile, EngineError> {
        self.device.profile.ok_or_else(|| EngineError::Unsupported {
            identity: self.device.identity.hex(),
        })
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
        match decode(&bytes) {
            Ok(message) => Ok(Some(message)),
            Err(error) => {
                tracing::warn!(%error, "dropping invalid message");
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
        self.transport.send(&request.encode())?;
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

    pub(crate) fn read_temp(&mut self) -> Result<PatchImage, EngineError> {
        let profile = self.profile()?;
        let started = self.clock.now();
        let packets = self.request(profile.read_temp_request())?;
        let image = PatchImage::from_packets(&profile.patch_layout, profile.temp_patch, &packets)?;
        tracing::debug!(
            packets = packets.len(),
            millis = self.clock.now().duration_since(started).as_millis(),
            "temp read"
        );
        Ok(image)
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
            self.transport.send(&packet.encode())?;
            self.drain(self.timings.packet_gap)?;
        }
        Ok(())
    }

    fn editor_mode_on(&mut self, sink: &mut dyn FnMut(Event)) -> Result<(), EngineError> {
        let profile = self.profile()?;
        sink(Event::Progress(Stage::EditorMode));
        self.send_packets(&[profile.editor_mode_on()?])
    }

    pub(crate) fn write_verified(
        &mut self,
        target: &PatchImage,
        offsets: Option<&[PatchOffset]>,
        sink: &mut dyn FnMut(Event),
    ) -> Result<WriteOutcome, EngineError> {
        let profile = self.profile()?;
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
            sink(Event::Progress(Stage::ProgramChange { program, attempt }));
            self.transport.send(&program_change(program))?;
            self.drain(self.timings.channel_settle)?;
            let current = self.read_current_channel()?;
            tracing::info!(%current, attempt, "channel readback");
            actual = Some(current);
            if current == channel {
                self.drain(self.timings.channel_settle)?;
                let temp = self.read_temp()?;
                return Ok(SwitchOutcome {
                    channel,
                    program,
                    attempts: attempt,
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
}
