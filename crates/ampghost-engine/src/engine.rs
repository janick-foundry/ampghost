use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ampghost_midi::{MidiBackend, PortInfo, PortWatch, katana_candidates};
use ampghost_params::{ParamTable, ParamsError};
use ampghost_protocol::{
    Channel, DeviceProfile, Dt1, KATANA_50_MK2, LinkedType, PatchImage, PatchOffset, ProtocolError,
    verify,
};
use ampghost_store::{
    FullBackup, LiveSetPatch, NamedBlock, Preset, PresetStorage, StoreError, TSL_MAX_PATCHES,
    blocks_to_image, decode_preset, decode_syx, encode_preset, encode_syx, encode_tsl,
    image_to_blocks, preset_key, read_preset_summary, validate_key,
};

use crate::clock::Clock;
use crate::command::{
    ChainOutcome, Command, DetectReport, Event, ParamWrite, PresetEntry, Stage, SwitchOutcome,
    WriteOutcome,
};
use crate::error::EngineError;
use crate::session::Session;
use crate::timings::Timings;

const OFFLINE_PROFILE: &DeviceProfile = &KATANA_50_MK2;
const EDITOR_OFF_GRACE: Duration = Duration::from_millis(500);

pub struct Engine {
    backend: Box<dyn MidiBackend>,
    clock: Arc<dyn Clock>,
    timings: Timings,
    presets: Box<dyn PresetStorage>,
    backups: Box<dyn PresetStorage>,
    session: Option<Session>,
    lost: bool,
    auto_connect: bool,
    next_port_check: Option<Instant>,
    probed: HashMap<String, u8>,
    watch: Option<PortWatch>,
}

impl Engine {
    pub fn new(
        backend: Box<dyn MidiBackend>,
        clock: Arc<dyn Clock>,
        presets: Box<dyn PresetStorage>,
        backups: Box<dyn PresetStorage>,
    ) -> Self {
        Self {
            backend,
            clock,
            timings: Timings::default(),
            presets,
            backups,
            session: None,
            lost: false,
            auto_connect: false,
            next_port_check: None,
            probed: HashMap::new(),
            watch: None,
        }
    }

    pub fn watch_ports(&mut self, notify: Box<dyn Fn() + Send>) {
        match self.backend.watch(notify) {
            Ok(watch) => self.watch = Some(watch),
            Err(error) => tracing::warn!(
                error = &error as &dyn std::error::Error,
                "MIDI port announcements unavailable, checking ports periodically"
            ),
        }
    }

    pub fn ports_changed(&mut self, sink: &mut dyn FnMut(Event)) {
        self.next_port_check = Some(self.clock.now() + self.timings.port_check);
        let ports = match self.backend.ports() {
            Ok(ports) => ports,
            Err(error) => {
                tracing::debug!(
                    error = &error as &dyn std::error::Error,
                    "listing MIDI ports failed"
                );
                return;
            }
        };
        if let Some(session) = self.session.as_ref() {
            let port = session.device().port.clone();
            let listed = ports.iter().any(|candidate| candidate.name == port);
            if !listed || !session.is_present() {
                self.disconnect(sink);
            }
            return;
        }
        if self.auto_connect {
            self.connect_new(&ports, sink);
        }
    }

    fn disconnect(&mut self, sink: &mut dyn FnMut(Event)) {
        let Some(session) = self.session.take() else {
            return;
        };
        let port = session.device().port.clone();
        drop(session);
        self.lost = true;
        self.probed.clear();
        tracing::info!(%port, "Katana disconnected");
        sink(Event::Disconnected { port });
    }

    fn connect_new(&mut self, ports: &[PortInfo], sink: &mut dyn FnMut(Event)) {
        let candidates = katana_candidates(ports);
        self.probed
            .retain(|name, _| candidates.iter().any(|port| &port.name == name));
        for port in candidates {
            let attempts = self.probed.entry(port.name.clone()).or_insert(0);
            if *attempts >= self.timings.connect_attempts {
                continue;
            }
            *attempts += 1;
            tracing::info!(port = %port.name, attempt = *attempts, "new MIDI port, probing for a Katana");
            match self.probe(&port) {
                Ok(report) => {
                    self.lost = false;
                    self.probed.clear();
                    let packets = self
                        .session
                        .as_mut()
                        .map_or_else(Vec::new, Session::take_unsolicited);
                    for packet in packets {
                        sink(Event::Unsolicited(packet));
                    }
                    tracing::info!(port = %port.name, "Katana connected");
                    sink(Event::Reconnected(report));
                    return;
                }
                Err(error) => {
                    self.session = None;
                    tracing::warn!(port = %port.name, error = &error as &dyn std::error::Error, "probe failed");
                }
            }
        }
    }

    pub fn with_timings(mut self, timings: Timings) -> Self {
        self.timings = timings;
        self
    }

    pub fn handle(&mut self, command: Command, sink: &mut dyn FnMut(Event)) {
        let follow = !matches!(command, Command::Detect | Command::SwitchChannel(_));
        let detect = matches!(command, Command::Detect);
        if detect {
            self.lost = false;
            self.probed.clear();
        }
        let result = self.run(command, sink);
        if detect {
            self.auto_connect = true;
        }
        let gone = result.as_ref().is_err_and(EngineError::is_disconnect);
        let packets = self
            .session
            .as_mut()
            .map_or_else(Vec::new, Session::take_unsolicited);
        let deferred = if follow {
            packets
        } else {
            for packet in packets {
                sink(Event::Unsolicited(packet));
            }
            Vec::new()
        };
        match result {
            Ok(event) => sink(event),
            Err(error) => {
                tracing::error!(error = &error as &dyn std::error::Error, "command failed");
                sink(Event::Failed(error));
            }
        }
        if gone {
            self.disconnect(sink);
            return;
        }
        self.forward_unsolicited(deferred, sink);
    }

    pub fn poll(&mut self, window: Duration, sink: &mut dyn FnMut(Event)) {
        if let Some(session) = self.session.as_mut() {
            match session.drain(window) {
                Err(error) if error.is_disconnect() => {
                    self.disconnect(sink);
                    return;
                }
                Err(error) => {
                    tracing::debug!(error = &error as &dyn std::error::Error, "idle poll failed");
                }
                Ok(()) => {}
            }
            let packets = session.take_unsolicited();
            self.forward_unsolicited(packets, sink);
        }
        let due = self
            .next_port_check
            .is_none_or(|next| self.clock.now() >= next);
        if due && (self.session.is_some() || self.auto_connect) {
            self.ports_changed(sink);
        }
    }

    fn forward_unsolicited(&mut self, packets: Vec<Dt1>, sink: &mut dyn FnMut(Event)) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let switched = session.follow_amp_switch(&packets);
        for packet in packets.into_iter().chain(session.take_unsolicited()) {
            sink(Event::Unsolicited(packet));
        }
        match switched {
            Ok(Some(switch)) => sink(Event::AmpSwitched(switch)),
            Ok(None) => {}
            Err(error) => tracing::warn!(
                error = &error as &dyn std::error::Error,
                "could not follow the amp's channel change"
            ),
        }
    }

    fn run(&mut self, command: Command, sink: &mut dyn FnMut(Event)) -> Result<Event, EngineError> {
        match command {
            Command::Detect => {
                self.session = None;
                self.detect().map(Event::Detected)
            }
            Command::ReadTemp => self.session()?.read_temp().map(Event::TempRead),
            Command::WriteTemp(image) => self
                .session()?
                .write_verified(&image, None, sink)
                .map(Event::WriteVerified),
            Command::WriteParams(writes) => {
                self.write_params(&writes, sink).map(Event::WriteVerified)
            }
            Command::WriteChain(pattern) => {
                self.write_chain(pattern, sink).map(Event::ChainWritten)
            }
            Command::SwitchChannel(channel) => self
                .switch_channel(channel, sink)
                .map(Event::ChannelSwitched),
            Command::BackupTemp { key } => self.backup_temp(key),
            Command::ReadFullBackup => self.read_full_backup(sink).map(Event::FullBackupRead),
            Command::RestoreBackup { key } => self.restore_backup(key, sink),
            Command::SavePreset { name } => self.save_preset(name),
            Command::SaveImportedPreset { name, image } => self.save_imported_preset(name, image),
            Command::ExportLiveset { keys, name } => self.export_liveset(&keys, &name),
            Command::ApplyPreset { key } => self.apply_preset(key, sink),
            Command::UpdatePreset { key, name } => self.update_preset(key, name),
            Command::DeletePreset { key } => {
                validate_key(&key)?;
                self.presets.delete(&key)?;
                Ok(Event::PresetDeleted { key })
            }
            Command::ListPresets => self.list_presets().map(Event::Presets),
            Command::ListBackups => Ok(Event::Backups(self.backups.list()?)),
            Command::Shutdown => {
                self.release_amp();
                self.watch = None;
                Ok(Event::ShutDown)
            }
        }
    }

    fn release_amp(&mut self) {
        let Some(mut session) = self.session.take() else {
            return;
        };
        match session.leave_editor_mode() {
            Ok(true) => {
                tracing::info!(port = %session.device().port, "editor mode off sent");
                self.clock.sleep(EDITOR_OFF_GRACE);
            }
            Ok(false) => {}
            Err(error) => tracing::warn!(
                error = &error as &dyn std::error::Error,
                "could not turn editor mode off"
            ),
        }
    }

    fn detect(&mut self) -> Result<DetectReport, EngineError> {
        let ports = self.backend.ports()?;
        let named = katana_candidates(&ports);
        let candidates = if named.is_empty() { ports } else { named };
        let mut last = None;
        for port in candidates {
            tracing::info!(port = %port.name, "probing");
            match self.probe(&port) {
                Ok(report) => return Ok(report),
                Err(error) if error.is_disconnect() => return Err(error),
                Err(error) => {
                    self.session = None;
                    tracing::warn!(port = %port.name, error = &error as &dyn std::error::Error, "probe failed");
                    last = Some(Box::new(error));
                }
            }
        }
        Err(EngineError::NotFound { last })
    }

    fn probe(&mut self, port: &PortInfo) -> Result<DetectReport, EngineError> {
        let transport = self.backend.open(port)?;
        let session = Session::identify(transport, Arc::clone(&self.clock), self.timings)?;
        let session = self.session.insert(session);
        let device = session.device().clone();
        if !device.is_supported() {
            return Ok(DetectReport {
                device,
                current_channel: None,
                program_change_map: None,
            });
        }
        let current_channel = Some(session.read_current_channel()?);
        let program_change_map = Some(session.read_program_change_map()?);
        session.enter_editor_mode()?;
        Ok(DetectReport {
            device,
            current_channel,
            program_change_map,
        })
    }

    fn session(&mut self) -> Result<&mut Session, EngineError> {
        if self.session.is_none() && self.lost {
            return Err(EngineError::Disconnected);
        }
        if self.session.is_none() {
            self.detect()?;
        }
        let session = self
            .session
            .as_mut()
            .ok_or(EngineError::NotFound { last: None })?;
        session.profile()?;
        Ok(session)
    }

    fn write_params(
        &mut self,
        writes: &[ParamWrite],
        sink: &mut dyn FnMut(Event),
    ) -> Result<WriteOutcome, EngineError> {
        let table = ampghost_params::table()?;
        let session = self.session()?;
        let links = session.profile()?.verify_rules.linked_types;
        let current = session.read_temp()?;
        let target = apply_param_writes(table, &current, writes, links)?;
        let changed: Vec<PatchOffset> = target.differing_offsets(&current);
        if changed.is_empty() {
            return Ok(WriteOutcome {
                attempts: 0,
                packets: 0,
                report: ampghost_protocol::VerifyReport::default(),
                image: current,
            });
        }
        session.write_verified(&target, Some(&changed), sink)
    }

    fn write_chain(
        &mut self,
        pattern: u8,
        sink: &mut dyn FnMut(Event),
    ) -> Result<ChainOutcome, EngineError> {
        let session = self.session()?;
        let profile = session.profile()?;
        let chain_start = profile.write_order.effect_chain.start().linear();
        let pattern_at = profile.chain_pattern.linear();
        let current = session.read_temp()?;
        let target = profile.with_chain_pattern(&current, pattern)?;
        let previous = profile
            .chain_pattern_of(&current)
            .ok_or(ProtocolError::OutsideLayout(pattern_at))?;
        if previous == pattern {
            return Ok(ChainOutcome {
                previous,
                requested: pattern,
                actual: previous,
                order: profile
                    .chain_order(&current)
                    .ok_or(ProtocolError::OutsideLayout(chain_start))?,
                packets: 0,
                report: ampghost_protocol::VerifyReport::default(),
                image: current,
            });
        }
        tracing::info!(from = previous, to = pattern, "writing chain pattern");
        let (packets, image) = session.write_chain(&target, sink)?;
        let actual = profile
            .chain_pattern_of(&image)
            .ok_or(ProtocolError::OutsideLayout(pattern_at))?;
        let order = profile
            .chain_order(&image)
            .ok_or(ProtocolError::OutsideLayout(chain_start))?;
        let report = verify(&target, &image, &profile.verify_rules);
        if actual == pattern {
            tracing::info!(packets, order = %hex(&order), "chain pattern verified");
        } else {
            tracing::warn!(
                requested = pattern,
                actual,
                "amp did not keep the chain pattern"
            );
        }
        if !report.is_ok() {
            let differing = report
                .mismatches
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>();
            tracing::warn!(bytes = differing.len(), differing = %differing.join(", "), "amp changed other bytes after the chain write");
        }
        Ok(ChainOutcome {
            previous,
            requested: pattern,
            actual,
            order,
            packets,
            report,
            image,
        })
    }

    fn list_presets(&self) -> Result<Vec<PresetEntry>, EngineError> {
        let mut entries = Vec::new();
        for key in self.presets.list()? {
            let summary = self
                .presets
                .read(&key)
                .ok()
                .and_then(|bytes| String::from_utf8(bytes).ok())
                .and_then(|text| read_preset_summary(&text).ok());
            let entry = if let Some(summary) = summary {
                PresetEntry {
                    key,
                    name: summary.name,
                    model: summary.model,
                    created: summary.created,
                    patch_name: summary.patch_name,
                }
            } else {
                tracing::warn!(%key, "unreadable preset file");
                PresetEntry {
                    name: key.trim_end_matches(".json").to_owned(),
                    key,
                    model: String::new(),
                    created: String::new(),
                    patch_name: String::new(),
                }
            };
            entries.push(entry);
        }
        entries.sort_by_key(|entry| entry.name.to_lowercase());
        Ok(entries)
    }

    fn backup_temp(&mut self, key: String) -> Result<Event, EngineError> {
        validate_key(&key)?;
        let session = self.session()?;
        let profile = session.profile()?;
        let image = session.read_temp()?;
        let bytes = encode_syx(&image, profile)?;
        self.backups.write(&key, &bytes)?;
        Ok(Event::BackupSaved { key, image })
    }

    fn switch_channel(
        &mut self,
        channel: Channel,
        sink: &mut dyn FnMut(Event),
    ) -> Result<SwitchOutcome, EngineError> {
        let session = self.session()?;
        let profile = session.profile()?;
        if !profile.has_channel(channel) {
            return Err(EngineError::ChannelUnavailable(channel));
        }
        let current = session.read_current_channel()?;
        tracing::info!(%current, %channel, "channel before switch");
        if current != channel {
            return session.switch_channel(channel, sink);
        }
        sink(Event::Progress(Stage::Reloading { channel }));
        if let Some(row) = profile.reload_row(channel) {
            match self.reload_stored(channel, row, sink) {
                Ok(outcome) => return Ok(outcome),
                Err(error) if error.is_disconnect() => return Err(error),
                Err(error) => {
                    tracing::warn!(%channel, row, error = &error as &dyn std::error::Error, "stored reload failed, bouncing");
                }
            }
        }
        self.session()?.bounce_channel(channel, sink)
    }

    fn reload_stored(
        &mut self,
        channel: Channel,
        row: u8,
        sink: &mut dyn FnMut(Event),
    ) -> Result<SwitchOutcome, EngineError> {
        let stamp: String = self
            .clock
            .timestamp()
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect();
        let key = format!("auto-before-reload-{stamp}.syx");
        validate_key(&key)?;
        let session = self.session()?;
        let profile = session.profile()?;
        let layout = &profile.backup;
        let base = layout.stored_patch_base(row)?;
        let mut requests = Vec::new();
        for block in layout.patch_blocks {
            let chunks = layout.chunk_requests(block.offset.address(base)?, block.size)?;
            requests.push((block, chunks));
        }
        let total = requests.iter().map(|(_, chunks)| chunks.len()).sum();
        let mut done = 0;
        let mut blocks = Vec::new();
        for (block, chunks) in requests {
            let mut data = Vec::new();
            for request in chunks {
                sink(Event::Progress(Stage::ReadingStored {
                    channel,
                    done,
                    total,
                }));
                data.extend(session.read_exact(request)?);
                done += 1;
            }
            blocks.push(NamedBlock {
                name: block.name.to_owned(),
                data,
            });
        }
        let (mut image, report) = blocks_to_image(&blocks, &profile.patch_layout, layout)?;
        tracing::info!(%channel, row, name = %image.name(), ?report, "stored patch read");
        sink(Event::Progress(Stage::ReadingTemp));
        let previous = session.read_temp()?;
        if let Some(status) = layout.patch_block(layout.status_block) {
            for offset in status.span().offsets() {
                if let Some(value) = previous.get(offset) {
                    image.set(offset, value)?;
                }
            }
        }
        let bytes = encode_syx(&previous, profile)?;
        self.backups.write(&key, &bytes)?;
        tracing::info!(%key, "temp backed up before reload");
        let session = self.session()?;
        let program = session.read_program_change_map()?.program(channel);
        let written = session.write_verified(&image, None, sink)?;
        Ok(SwitchOutcome {
            channel,
            program,
            attempts: 1,
            reloaded: true,
            temp: written.image,
        })
    }

    fn read_full_backup(&mut self, sink: &mut dyn FnMut(Event)) -> Result<FullBackup, EngineError> {
        let session = self.session()?;
        let layout = session.profile()?.backup;
        let mut regions = Vec::new();
        for block in layout.system_blocks {
            regions.push((None, block.name, block.address, block.size));
        }
        for row in 0..layout.stored_patch_slots {
            if !layout.stored_patch_rows.contains(&row) {
                continue;
            }
            let base = layout.stored_patch_base(row)?;
            for block in layout.patch_blocks {
                regions.push((
                    Some(row),
                    block.name,
                    block.offset.address(base)?,
                    block.size,
                ));
            }
        }
        let mut requests = Vec::new();
        for &(_, _, address, size) in &regions {
            requests.push(layout.chunk_requests(address, size)?);
        }
        let total = requests.iter().map(Vec::len).sum();
        let mut done = 0;
        let mut system = Vec::new();
        let mut rows: Vec<Option<Vec<NamedBlock>>> =
            vec![None; usize::from(layout.stored_patch_slots)];
        for (&(row, name, _, _), chunks) in regions.iter().zip(requests) {
            let mut data = Vec::new();
            for request in chunks {
                sink(Event::Progress(Stage::ReadingBackup { done, total }));
                data.extend(session.read_exact(request)?);
                done += 1;
            }
            let block = NamedBlock {
                name: name.to_owned(),
                data,
            };
            match row.and_then(|row| rows.get_mut(usize::from(row))) {
                Some(slot) => slot.get_or_insert_with(Vec::new).push(block),
                None => system.push(block),
            }
        }
        sink(Event::Progress(Stage::ReadingBackup { done, total }));
        Ok(FullBackup {
            system,
            user_patches: rows,
        })
    }

    fn load_backup(&mut self, key: &str) -> Result<PatchImage, EngineError> {
        validate_key(key)?;
        let bytes = self.backups.read(key)?;
        let profile = self.session()?.profile()?;
        Ok(decode_syx(&bytes, profile)?)
    }

    fn restore_backup(
        &mut self,
        key: String,
        sink: &mut dyn FnMut(Event),
    ) -> Result<Event, EngineError> {
        let image = self.load_backup(&key)?;
        let outcome = self.session()?.write_verified(&image, None, sink)?;
        Ok(Event::Restored { key, outcome })
    }

    fn save_preset(&mut self, name: String) -> Result<Event, EngineError> {
        let key = preset_key(&name)?;
        let timestamp = self.clock.timestamp();
        let session = self.session()?;
        let profile = session.profile()?;
        let mut image = session.read_temp()?;
        ampghost_params::conform_to_official(&mut image)?;
        let preset = Preset::new(name.clone(), profile.name, timestamp, image)?;
        let text = encode_preset(&preset, profile.temp_patch)?;
        self.presets.write(&key, text.as_bytes())?;
        Ok(Event::PresetSaved { key, name })
    }

    fn save_imported_preset(
        &mut self,
        name: String,
        mut image: PatchImage,
    ) -> Result<Event, EngineError> {
        let key = preset_key(&name)?;
        let profile = OFFLINE_PROFILE;
        ampghost_params::conform_to_official(&mut image)?;
        let preset = Preset::new(name.clone(), profile.name, self.clock.timestamp(), image)?;
        let text = encode_preset(&preset, profile.temp_patch)?;
        self.presets.write(&key, text.as_bytes())?;
        Ok(Event::PresetSaved { key, name })
    }

    fn export_liveset(&self, keys: &[String], name: &str) -> Result<Event, EngineError> {
        if keys.is_empty() {
            return Err(StoreError::NoPatches.into());
        }
        if keys.len() > TSL_MAX_PATCHES {
            return Err(StoreError::TooManyPatches(keys.len(), TSL_MAX_PATCHES).into());
        }
        let profile = OFFLINE_PROFILE;
        let mut patches = Vec::new();
        for key in keys {
            validate_key(key)?;
            let text = String::from_utf8(self.presets.read(key)?)
                .map_err(|_| EngineError::PresetEncoding)?;
            let preset = decode_preset(&text, &profile.patch_layout, profile.temp_patch)?;
            patches.push(LiveSetPatch {
                memo: String::new(),
                blocks: image_to_blocks(&preset.image, &profile.backup)?,
            });
        }
        let text = encode_tsl(name, &patches)?;
        Ok(Event::LivesetEncoded {
            text,
            patches: patches.len(),
        })
    }

    fn update_preset(&mut self, previous: String, name: String) -> Result<Event, EngineError> {
        validate_key(&previous)?;
        let existing = String::from_utf8(self.presets.read(&previous)?)
            .map_err(|_| EngineError::PresetEncoding)?;
        let created = read_preset_summary(&existing)
            .map_or_else(|_| self.clock.timestamp(), |summary| summary.created);
        let key = preset_key(&name)?;
        if key != previous && self.presets.list()?.contains(&key) {
            return Err(StoreError::AlreadyExists(key).into());
        }
        let session = self.session()?;
        let profile = session.profile()?;
        let mut image = session.read_temp()?;
        ampghost_params::conform_to_official(&mut image)?;
        let preset = Preset::new(name.clone(), profile.name, created, image)?;
        let text = encode_preset(&preset, profile.temp_patch)?;
        self.presets.write(&key, text.as_bytes())?;
        if key != previous {
            self.presets.delete(&previous)?;
        }
        Ok(Event::PresetUpdated {
            previous,
            key,
            name,
        })
    }

    fn apply_preset(
        &mut self,
        key: String,
        sink: &mut dyn FnMut(Event),
    ) -> Result<Event, EngineError> {
        validate_key(&key)?;
        let bytes = self.presets.read(&key)?;
        let text = String::from_utf8(bytes).map_err(|_| EngineError::PresetEncoding)?;
        let session = self.session()?;
        let profile = session.profile()?;
        let preset = decode_preset(&text, &profile.patch_layout, profile.temp_patch)?;
        if preset.model != profile.name {
            tracing::warn!(model = %preset.model, "preset was saved for a different model");
        }
        let outcome = session.write_verified(&preset.image, None, sink)?;
        Ok(Event::PresetApplied {
            key,
            name: preset.name,
            outcome,
        })
    }
}

pub fn apply_param_writes(
    table: &ParamTable,
    current: &PatchImage,
    writes: &[ParamWrite],
    links: &[LinkedType],
) -> Result<PatchImage, EngineError> {
    let mut target = current.clone();
    for write in writes {
        let param = table.get(&write.id)?;
        if param.is_hidden() {
            return Err(ParamsError::Hidden(write.id.clone()).into());
        }
        let offset = param.patch_offset()?;
        for (distance, byte) in (0_u32..).zip(param.encode(write.value)?) {
            target.set(offset.plus(distance), byte)?;
        }
        tracing::info!(id = %write.id, value = write.value, %offset, "param");
    }
    link_variant_types(current, &mut target, links)?;
    Ok(target)
}

fn link_variant_types(
    current: &PatchImage,
    target: &mut PatchImage,
    links: &[LinkedType],
) -> Result<(), EngineError> {
    for link in links {
        let Some(variant_at) = link.active_variant(target) else {
            continue;
        };
        if current.get(link.select_at) != target.get(link.select_at) {
            if let Some(value) = target.get(variant_at) {
                target.set(link.type_at, value)?;
                tracing::info!(
                    block = link.block,
                    value,
                    "variant selected, block type follows"
                );
            }
        } else if current.get(link.type_at) != target.get(link.type_at)
            && let Some(value) = target.get(link.type_at)
        {
            target.set(variant_at, value)?;
            tracing::info!(block = link.block, value, %variant_at, "block type stored in active variant");
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}
