use std::sync::Arc;

use katana_midi::{MidiBackend, katana_candidates};
use katana_protocol::{PatchImage, PatchOffset};
use katana_store::{
    Preset, PresetStorage, decode_preset, decode_syx, encode_preset, encode_syx, preset_key,
    validate_key,
};

use crate::clock::Clock;
use crate::command::{Command, DetectReport, Event, ParamWrite, WriteOutcome};
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
        let result = self.run(command, sink);
        if let Some(session) = self.session.as_mut() {
            for packet in session.take_unsolicited() {
                sink(Event::Unsolicited(packet));
            }
        }
        match result {
            Ok(event) => sink(event),
            Err(error) => {
                tracing::error!(%error, "command failed");
                sink(Event::Failed(error));
            }
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
            Command::SwitchChannel(channel) => self
                .session()?
                .switch_channel(channel, sink)
                .map(Event::ChannelSwitched),
            Command::BackupTemp { key } => self.backup_temp(key),
            Command::RestoreBackup { key } => self.restore_backup(key, sink),
            Command::SavePreset { name } => self.save_preset(name),
            Command::ApplyPreset { key } => self.apply_preset(key, sink),
            Command::ListPresets => Ok(Event::Presets(self.presets.list()?)),
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
        let current = session.read_temp()?;
        let mut target = current.clone();
        for write in writes {
            let param = table.get(&write.id)?;
            let offset = param.patch_offset()?;
            for (distance, byte) in (0_u32..).zip(param.encode(write.value)?) {
                target.set(offset.plus(distance), byte)?;
            }
            tracing::info!(id = %write.id, value = write.value, %offset, "param");
        }
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
