use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use katana_store::{PresetStorage, StoreError, validate_key};

use crate::APP_ID;
use crate::clock::Clock;
use crate::command::{Command, Event};
use crate::engine::Engine;
use crate::error::EngineError;

const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(200);
const IDLE_POLL_WINDOW: Duration = Duration::from_millis(5);

#[derive(Debug, Default, Clone, Copy)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn sleep(&self, duration: Duration) {
        thread::sleep(duration);
    }

    fn timestamp(&self) -> String {
        humantime::format_rfc3339_seconds(SystemTime::now()).to_string()
    }
}

pub fn default_data_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from_path(PathBuf::from(APP_ID))
        .map(|dirs| dirs.data_dir().to_path_buf())
}

#[derive(Debug, Clone)]
pub struct FileStorage {
    root: PathBuf,
    extension: Option<String>,
}

impl FileStorage {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            extension: None,
        }
    }

    pub fn with_extension(mut self, extension: &str) -> Self {
        self.extension = Some(extension.to_owned());
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn path(&self, key: &str) -> Result<PathBuf, StoreError> {
        validate_key(key)?;
        Ok(self.root.join(key))
    }
}

fn storage_error(error: &std::io::Error) -> StoreError {
    StoreError::Storage(error.to_string())
}

impl PresetStorage for FileStorage {
    fn list(&self) -> Result<Vec<String>, StoreError> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(storage_error(&error)),
        };
        let mut keys = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|error| storage_error(&error))?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let wanted = self
                .extension
                .as_ref()
                .is_none_or(|extension| name.ends_with(extension.as_str()));
            if wanted && validate_key(&name).is_ok() && entry.path().is_file() {
                keys.push(name);
            }
        }
        keys.sort();
        Ok(keys)
    }

    fn read(&self, key: &str) -> Result<Vec<u8>, StoreError> {
        let path = self.path(key)?;
        fs::read(&path).map_err(|error| match error.kind() {
            ErrorKind::NotFound => StoreError::NotFound(path.display().to_string()),
            _ => storage_error(&error),
        })
    }

    fn write(&mut self, key: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let path = self.path(key)?;
        fs::create_dir_all(&self.root).map_err(|error| storage_error(&error))?;
        let temporary = self.root.join(format!(".{key}.partial"));
        let mut file = fs::File::create(&temporary).map_err(|error| storage_error(&error))?;
        file.write_all(bytes)
            .map_err(|error| storage_error(&error))?;
        file.sync_all().map_err(|error| storage_error(&error))?;
        fs::rename(&temporary, &path).map_err(|error| storage_error(&error))?;
        tracing::info!(path = %path.display(), bytes = bytes.len(), "stored");
        Ok(())
    }
}

pub struct EngineHandle {
    commands: mpsc::Sender<Command>,
    thread: Option<thread::JoinHandle<()>>,
}

impl EngineHandle {
    pub fn send(&self, command: Command) -> Result<(), EngineError> {
        self.commands
            .send(command)
            .map_err(|_| EngineError::Stopped)
    }

    pub fn shutdown(mut self) {
        if self.commands.send(Command::Shutdown).is_err() {
            tracing::debug!("engine already stopped");
        }
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            tracing::error!("engine thread panicked");
        }
    }
}

pub fn spawn(
    mut engine: Engine,
    mut sink: Box<dyn FnMut(Event) + Send>,
) -> std::io::Result<EngineHandle> {
    let (commands, receiver) = mpsc::channel::<Command>();
    let thread = thread::Builder::new()
        .name("katana-engine".to_owned())
        .spawn(move || {
            loop {
                match receiver.recv_timeout(IDLE_POLL_INTERVAL) {
                    Ok(command) => {
                        let stop = matches!(command, Command::Shutdown);
                        engine.handle(command, &mut *sink);
                        if stop {
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        engine.poll(IDLE_POLL_WINDOW, &mut *sink);
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        })?;
    Ok(EngineHandle {
        commands,
        thread: Some(thread),
    })
}
