use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use katana_engine::{
    Clock, Command, Engine, EngineHandle, Event, FileStorage, ParamWrite, SystemClock,
    default_data_dir, spawn,
};
use katana_midi::MidirBackend;
use katana_protocol::{DeviceProfile, KATANA_50_MK2, PatchImage};
use katana_store::{decode_syx, encode_syx, preset_key};

use crate::cli::{Action, Args};
use crate::output::{print_detect, print_dump, print_image, print_outcome, sha256_hex};

const EVENT_TIMEOUT: Duration = Duration::from_secs(60);
const PROFILE: &DeviceProfile = &KATANA_50_MK2;

pub fn run(args: Args) -> Result<()> {
    let data_dir = match args.data_dir {
        Some(dir) => dir,
        None => default_data_dir().context("no user data directory available")?,
    };
    let (backup_root, backup_key) = backup_location(&args.command, &data_dir)?;
    let presets = FileStorage::new(data_dir.join("presets")).with_extension(".json");
    let backups = FileStorage::new(backup_root).with_extension(".syx");
    let engine = Engine::new(
        Box::new(MidirBackend::new()),
        Arc::new(SystemClock),
        Box::new(presets),
        Box::new(backups.clone()),
    );
    let (sender, events) = mpsc::channel();
    let handle = spawn(
        engine,
        Box::new(move |event| {
            if sender.send(event).is_err() {
                tracing::debug!("event receiver gone");
            }
        }),
    )
    .context("starting engine thread")?;
    let mut driver = Driver { handle, events };
    let result = driver.execute(args.command, backup_key, &backups);
    driver.handle.shutdown();
    result
}

fn backup_location(action: &Action, data_dir: &Path) -> Result<(PathBuf, Option<String>)> {
    let (Action::Backup { out: Some(path) } | Action::Restore { file: path }) = action else {
        return Ok((data_dir.join("backups"), None));
    };
    let key = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .context("backup path needs a file name")?;
    let root = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    Ok((root, Some(key)))
}

struct Driver {
    handle: EngineHandle,
    events: mpsc::Receiver<Event>,
}

impl Driver {
    fn request(&mut self, command: Command) -> Result<Event> {
        self.handle.send(command)?;
        loop {
            let event = self
                .events
                .recv_timeout(EVENT_TIMEOUT)
                .map_err(|_| anyhow!("engine did not answer"))?;
            match event {
                Event::Progress(stage) => tracing::info!(?stage, "progress"),
                Event::Unsolicited(packet) => tracing::info!(%packet, "unsolicited from amp"),
                Event::Failed(error) => return Err(error.into()),
                other => return Ok(other),
            }
        }
    }

    fn read_temp(&mut self) -> Result<PatchImage> {
        match self.request(Command::ReadTemp)? {
            Event::TempRead(image) => Ok(image),
            other => bail!("unexpected engine event {other:?}"),
        }
    }

    fn execute(
        &mut self,
        action: Action,
        backup_key: Option<String>,
        backups: &FileStorage,
    ) -> Result<()> {
        match action {
            Action::Detect => match self.request(Command::Detect)? {
                Event::Detected(report) => print_detect(&report),
                other => bail!("unexpected engine event {other:?}"),
            },
            Action::ReadTemp { dump } => {
                let image = self.read_temp()?;
                print_image(&image, &encode_syx(&image, PROFILE.temp_patch)?);
                if dump {
                    print_dump(&image.to_packets(PROFILE.temp_patch)?);
                }
            }
            Action::Backup { .. } => {
                let key = backup_key.unwrap_or_else(default_backup_key);
                match self.request(Command::BackupTemp { key })? {
                    Event::BackupSaved { key, image } => {
                        println!("backup:     {}", backups.root().join(&key).display());
                        print_image(&image, &encode_syx(&image, PROFILE.temp_patch)?);
                    }
                    other => bail!("unexpected engine event {other:?}"),
                }
            }
            Action::Restore { file } => {
                let key = backup_key.context("restore needs a file")?;
                self.restore(&file, key)?;
            }
            Action::WriteTemp { file } => {
                let bytes =
                    std::fs::read(&file).with_context(|| format!("reading {}", file.display()))?;
                let image = decode_syx(&bytes, &PROFILE.patch_layout, PROFILE.temp_patch)?;
                println!("writing:    {} ({})", file.display(), image.name());
                match self.request(Command::WriteTemp(image))? {
                    Event::WriteVerified(outcome) => print_outcome(&outcome),
                    other => bail!("unexpected engine event {other:?}"),
                }
            }
            Action::ReadParam { id } => self.read_param(&id)?,
            Action::WriteParam { id, value } => {
                match self.request(Command::WriteParams(vec![ParamWrite { id, value }]))? {
                    Event::WriteVerified(outcome) => print_outcome(&outcome),
                    other => bail!("unexpected engine event {other:?}"),
                }
            }
            Action::PresetSave { name } => match self.request(Command::SavePreset { name })? {
                Event::PresetSaved { key, name } => println!("saved preset {name:?} as {key}"),
                other => bail!("unexpected engine event {other:?}"),
            },
            Action::PresetApply { name } => {
                let key = if Path::new(&name)
                    .extension()
                    .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
                {
                    name
                } else {
                    preset_key(&name)?
                };
                match self.request(Command::ApplyPreset { key })? {
                    Event::PresetApplied { key, name, outcome } => {
                        println!("applied preset {name:?} ({key})");
                        print_outcome(&outcome);
                    }
                    other => bail!("unexpected engine event {other:?}"),
                }
            }
            Action::PresetList => match self.request(Command::ListPresets)? {
                Event::Presets(entries) => entries
                    .iter()
                    .for_each(|entry| println!("{}\t{}", entry.key, entry.name)),
                other => bail!("unexpected engine event {other:?}"),
            },
            Action::SwitchChannel {
                channel,
                discard_temp,
            } => {
                if !discard_temp {
                    bail!(
                        "switching the channel replaces the temp patch; pass --discard-temp to confirm"
                    );
                }
                let started = std::time::Instant::now();
                match self.request(Command::SwitchChannel(channel))? {
                    Event::ChannelSwitched(outcome) => {
                        println!(
                            "channel:    {} (PC {}, {} attempt(s), {} ms)",
                            outcome.channel,
                            outcome.program,
                            outcome.attempts,
                            started.elapsed().as_millis()
                        );
                        print_image(
                            &outcome.temp,
                            &encode_syx(&outcome.temp, PROFILE.temp_patch)?,
                        );
                    }
                    other => bail!("unexpected engine event {other:?}"),
                }
            }
        }
        Ok(())
    }

    fn restore(&mut self, file: &Path, key: String) -> Result<()> {
        let original =
            std::fs::read(file).with_context(|| format!("reading {}", file.display()))?;
        let wanted = decode_syx(&original, &PROFILE.patch_layout, PROFILE.temp_patch)?;
        println!("restoring:  {} ({})", file.display(), wanted.name());
        match self.request(Command::RestoreBackup { key })? {
            Event::Restored { outcome, .. } => print_outcome(&outcome),
            other => bail!("unexpected engine event {other:?}"),
        }
        let fresh = self.read_temp()?;
        let fresh_syx = encode_syx(&fresh, PROFILE.temp_patch)?;
        let file_sha = sha256_hex(&original);
        let fresh_sha = sha256_hex(&fresh_syx);
        println!("file sha256:  {file_sha}");
        println!("temp sha256:  {fresh_sha}");
        let differing = fresh.differing_offsets(&wanted);
        if differing.is_empty() && file_sha == fresh_sha {
            println!("restore:    temp is byte-identical to the backup");
            Ok(())
        } else {
            let list = differing
                .iter()
                .map(|offset| {
                    format!(
                        "{offset}: {:02X?} -> {:02X?}",
                        wanted.get(*offset),
                        fresh.get(*offset)
                    )
                })
                .collect::<Vec<_>>()
                .join(", ");
            bail!(
                "temp differs from the backup at {} byte(s): {list}",
                differing.len()
            )
        }
    }

    fn read_param(&mut self, id: &str) -> Result<()> {
        let param = katana_params::table()?.get(id)?;
        let offset = param.patch_offset()?;
        let image = self.read_temp()?;
        let bytes = (0..u32::from(param.size()))
            .map(|distance| image.get(offset.plus(distance)))
            .collect::<Option<Vec<u8>>>()
            .context("parameter lies outside the returned patch data")?;
        let value = param.decode(&bytes)?;
        let status = match param.status() {
            katana_params::Status::Research => "[R]".to_owned(),
            katana_params::Status::Verified { date } => format!("[V {date}]"),
        };
        println!(
            "{id} {status} at {offset}: raw {value} = {} ({} / {})",
            param.describe(value),
            param.block(),
            param.label()
        );
        Ok(())
    }
}

fn default_backup_key() -> String {
    let stamp = SystemClock.timestamp().replace(':', "-");
    format!("temp-{stamp}.syx")
}
