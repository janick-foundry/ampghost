use std::env;
use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use ampghost_store::{PresetStorage, StoreError, validate_key};

use crate::APP_ID;
use crate::clock::Clock;
use crate::command::{Command, Event};
use crate::engine::Engine;
use crate::error::EngineError;

const IDLE_POLL_INTERVAL: Duration = Duration::from_millis(200);
const IDLE_POLL_WINDOW: Duration = Duration::from_millis(5);
const SHUTDOWN_WAIT: Duration = Duration::from_secs(3);

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
        rfc3339_seconds(SystemTime::now())
    }
}

fn rfc3339_seconds(time: SystemTime) -> String {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    let (year, month, day) = civil_from_days(seconds / 86_400);
    let of_day = seconds % 86_400;
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        of_day / 3_600,
        of_day % 3_600 / 60,
        of_day % 60
    )
}

fn civil_from_days(days: u64) -> (u64, u64, u64) {
    let shifted = days + 719_468;
    let era = shifted / 146_097;
    let day_of_era = shifted % 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    };
    let year = era * 400 + year_of_era + u64::from(month <= 2);
    (year, month, day)
}

pub fn default_data_dir() -> Option<PathBuf> {
    let home = env::var_os("HOME")
        .filter(|home| !home.is_empty())
        .map(PathBuf::from)
        .or_else(env::home_dir)?;
    let base = env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"));
    Some(base.join(APP_ID))
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

fn storage_error(path: &Path) -> impl FnOnce(std::io::Error) -> StoreError + '_ {
    move |source| StoreError::Storage {
        path: path.to_path_buf(),
        source,
    }
}

impl PresetStorage for FileStorage {
    fn list(&self) -> Result<Vec<String>, StoreError> {
        let entries = match fs::read_dir(&self.root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => return Err(storage_error(&self.root)(error)),
        };
        let mut keys = Vec::new();
        for entry in entries {
            let entry = entry.map_err(storage_error(&self.root))?;
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
            _ => storage_error(&path)(error),
        })
    }

    fn write(&mut self, key: &str, bytes: &[u8]) -> Result<(), StoreError> {
        let path = self.path(key)?;
        fs::create_dir_all(&self.root).map_err(storage_error(&self.root))?;
        let temporary = self.root.join(format!(".{key}.partial"));
        let mut file = fs::File::create(&temporary).map_err(storage_error(&temporary))?;
        file.write_all(bytes).map_err(storage_error(&temporary))?;
        file.sync_all().map_err(storage_error(&temporary))?;
        fs::rename(&temporary, &path).map_err(storage_error(&path))?;
        tracing::info!(path = %path.display(), bytes = bytes.len(), "stored");
        Ok(())
    }

    fn delete(&mut self, key: &str) -> Result<(), StoreError> {
        let path = self.path(key)?;
        fs::remove_file(&path).map_err(|error| match error.kind() {
            ErrorKind::NotFound => StoreError::NotFound(path.display().to_string()),
            _ => storage_error(&path)(error),
        })?;
        tracing::info!(path = %path.display(), "deleted");
        Ok(())
    }
}

pub struct EngineHandle {
    commands: Option<mpsc::Sender<Command>>,
    finished: mpsc::Receiver<()>,
    thread: Option<thread::JoinHandle<()>>,
}

impl EngineHandle {
    pub fn send(&self, command: Command) -> Result<(), EngineError> {
        self.commands
            .as_ref()
            .ok_or(EngineError::Stopped)?
            .send(command)
            .map_err(|_| EngineError::Stopped)
    }

    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        let Some(commands) = self.commands.take() else {
            return;
        };
        if commands.send(Command::Shutdown).is_err() {
            tracing::debug!("engine already stopped");
        }
        drop(commands);
        let Some(thread) = self.thread.take() else {
            return;
        };
        match self.finished.recv_timeout(SHUTDOWN_WAIT) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                if thread.join().is_err() {
                    tracing::error!("engine thread panicked");
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                tracing::warn!(
                    seconds = SHUTDOWN_WAIT.as_secs(),
                    "engine did not stop in time, leaving it behind"
                );
            }
        }
    }
}

impl Drop for EngineHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

pub fn spawn(
    mut engine: Engine,
    mut sink: Box<dyn FnMut(Event) + Send>,
) -> std::io::Result<EngineHandle> {
    let (commands, receiver) = mpsc::channel::<Command>();
    let (finish, finished) = mpsc::channel::<()>();
    let ports_changed = Arc::new(AtomicBool::new(false));
    let notifier = Arc::clone(&ports_changed);
    engine.watch_ports(Box::new(move || notifier.store(true, Ordering::SeqCst)));
    let thread = thread::Builder::new()
        .name("ampghost-engine".to_owned())
        .spawn(move || {
            loop {
                let message = receiver.recv_timeout(IDLE_POLL_INTERVAL);
                if ports_changed.swap(false, Ordering::SeqCst) {
                    engine.ports_changed(&mut *sink);
                }
                match message {
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
            drop(engine);
            if finish.send(()).is_err() {
                tracing::debug!("engine handle gone before the engine stopped");
            }
        })?;
    Ok(EngineHandle {
        commands: Some(commands),
        finished,
        thread: Some(thread),
    })
}

#[cfg(test)]
mod tests;
