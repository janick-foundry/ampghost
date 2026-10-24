use std::sync::Arc;
use std::time::Duration;

use katana_midi::{MidiBackend, katana_candidates};
use katana_params::ParamTable;
use katana_protocol::{Dt1, LinkedType, PatchImage, PatchOffset, ProtocolError, verify};
use katana_store::{
    Preset, PresetStorage, decode_preset, decode_syx, encode_preset, encode_syx, preset_key,
    read_preset_summary, validate_key,
};

use crate::clock::Clock;
use crate::command::{
    ChainOutcome, Command, DetectReport, Event, ParamWrite, PresetEntry, WriteOutcome,
};
use crate::error::EngineError;
use crate::session::Session;
use crate::timings::Timings;

pub struct Engine {
    backend: Box<dyn MidiBackend>,
    clock: Arc<dyn Clock>,
    timings: Timings,
    presets: Box<dyn PresetStorage>,
    backups: Box<dyn PresetStorage>,
    session: Option<Session>,
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
        }
    }

    pub fn with_timings(mut self, timings: Timings) -> Self {
        self.timings = timings;
        self
    }

    pub fn handle(&mut self, command: Command, sink: &mut dyn FnMut(Event)) {
        let follow = !matches!(command, Command::Detect | Command::SwitchChannel(_));
        let result = self.run(command, sink);
        let packets = self
            .session
            .as_mut()
            .map(Session::take_unsolicited)
            .unwrap_or_default();
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
                tracing::error!(%error, "command failed");
                sink(Event::Failed(error));
            }
        }
        self.forward_unsolicited(deferred, sink);
    }

    pub fn poll(&mut self, window: Duration, sink: &mut dyn FnMut(Event)) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if let Err(error) = session.drain(window) {
            tracing::debug!(%error, "idle poll failed");
        }
        let packets = session.take_unsolicited();
        self.forward_unsolicited(packets, sink);
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
            Err(error) => tracing::warn!(%error, "could not follow the amp's channel change"),
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
            Command::WriteChain(order) => self.write_chain(order, sink).map(Event::ChainWritten),
            Command::SwitchChannel(channel) => self
                .session()?
                .switch_channel(channel, sink)
                .map(Event::ChannelSwitched),
            Command::BackupTemp { key } => self.backup_temp(key),
            Command::RestoreBackup { key } => self.restore_backup(key, sink),
            Command::SavePreset { name } => self.save_preset(name),
            Command::ApplyPreset { key } => self.apply_preset(key, sink),
            Command::ListPresets => self.list_presets().map(Event::Presets),
            Command::ListBackups => Ok(Event::Backups(self.backups.list()?)),
            Command::Shutdown => {
                self.session = None;
                Ok(Event::ShutDown)
            }
        }
    }

    fn detect(&mut self) -> Result<DetectReport, EngineError> {
        let ports = self.backend.ports()?;
        let named = katana_candidates(&ports);
        let candidates = if named.is_empty() { ports } else { named };
        let mut last = None;
        for port in candidates {
            tracing::info!(port = %port.name, "probing");
            let attempt = self
                .backend
                .open(&port)
                .map_err(EngineError::from)
                .and_then(|transport| {
                    Session::identify(transport, Arc::clone(&self.clock), self.timings)
                });
            match attempt {
                Ok(session) => {
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
                    return Ok(DetectReport {
                        device,
                        current_channel,
                        program_change_map,
                    });
                }
                Err(error) => {
                    tracing::warn!(port = %port.name, %error, "probe failed");
                    last = Some(error.to_string());
                }
            }
        }
        Err(EngineError::NotFound { last })
    }

    fn session(&mut self) -> Result<&mut Session, EngineError> {
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
        let table = katana_params::table()?;
        let session = self.session()?;
        let links = session.profile()?.verify_rules.linked_types;
        let current = session.read_temp()?;
        let target = apply_param_writes(table, &current, writes, links)?;
        let changed: Vec<PatchOffset> = target.differing_offsets(&current);
        if changed.is_empty() {
            return Ok(WriteOutcome {
                attempts: 0,
                packets: 0,
                report: katana_protocol::VerifyReport::default(),
                image: current,
            });
        }
        session.write_verified(&target, Some(&changed), sink)
    }

    fn write_chain(
        &mut self,
        order: Vec<u8>,
        sink: &mut dyn FnMut(Event),
    ) -> Result<ChainOutcome, EngineError> {
        let session = self.session()?;
        let profile = session.profile()?;
        profile.validate_chain(&order)?;
        let chain_start = profile.write_order.effect_chain.start().linear();
        let current = session.read_temp()?;
        let previous = profile
            .chain_order(&current)
            .ok_or(ProtocolError::OutsideLayout(chain_start))?;
        if previous == order {
            return Ok(ChainOutcome {
                actual: previous.clone(),
                previous,
                requested: order,
                packets: 0,
                report: katana_protocol::VerifyReport::default(),
                image: current,
            });
        }
        let target = profile.with_chain(&current, &order)?;
        tracing::info!(from = %hex(&previous), to = %hex(&order), "writing effect chain");
        let (packets, image) = session.write_chain(&target, sink)?;
        let actual = profile
            .chain_order(&image)
            .ok_or(ProtocolError::OutsideLayout(chain_start))?;
        let report = verify(&target, &image, &profile.verify_rules);
        if actual == order {
            tracing::info!(packets, "effect chain verified");
        } else {
            tracing::warn!(
                requested = %hex(&order),
                actual = %hex(&actual),
                "amp normalised the effect chain"
            );
        }
        let span = profile.write_order.effect_chain;
        let elsewhere = report
            .mismatches
            .iter()
            .filter(|mismatch| !span.contains(mismatch.offset))
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if !elsewhere.is_empty() {
            tracing::warn!(bytes = elsewhere.len(), differing = %elsewhere.join(", "), "amp changed other bytes after the chain write");
        }
        Ok(ChainOutcome {
            previous,
            requested: order,
            actual,
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
        let bytes = encode_syx(&image, profile.temp_patch)?;
        self.backups.write(&key, &bytes)?;
        Ok(Event::BackupSaved { key, image })
    }

    fn load_backup(&mut self, key: &str) -> Result<PatchImage, EngineError> {
        validate_key(key)?;
        let bytes = self.backups.read(key)?;
        let profile = self.session()?.profile()?;
        Ok(decode_syx(
            &bytes,
            &profile.patch_layout,
            profile.temp_patch,
        )?)
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
        let image = session.read_temp()?;
        let preset = Preset::new(name.clone(), profile.name, timestamp, image)?;
        let text = encode_preset(&preset, profile.temp_patch)?;
        self.presets.write(&key, text.as_bytes())?;
        Ok(Event::PresetSaved { key, name })
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
