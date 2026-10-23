use std::path::PathBuf;
use std::sync::{Arc, mpsc};

use anyhow::{Context, Result};
use katana_engine::{
    Engine, EngineHandle, Event, FileStorage, SystemClock, default_data_dir, spawn,
};
use katana_midi::MidirBackend;

pub struct Bridge {
    pub handle: EngineHandle,
    pub events: mpsc::Receiver<Event>,
    pub data_dir: PathBuf,
}

pub fn start() -> Result<Bridge> {
    let data_dir = default_data_dir().context("no user data directory available")?;
    let presets = FileStorage::new(data_dir.join("presets")).with_extension(".json");
    let backups = FileStorage::new(data_dir.join("backups")).with_extension(".syx");
    let engine = Engine::new(
        Box::new(MidirBackend::new()),
        Arc::new(SystemClock),
        Box::new(presets),
        Box::new(backups),
    );
    let (sender, events) = mpsc::channel();
    let handle = spawn(
        engine,
        Box::new(move |event| {
            if sender.send(event).is_err() {
                tracing::debug!("user interface is gone");
            }
        }),
    )
    .context("starting engine thread")?;
    Ok(Bridge {
        handle,
        events,
        data_dir,
    })
}
