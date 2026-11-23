use std::path::PathBuf;
use std::sync::Arc;

use ampghost_engine::{
    Engine, EngineHandle, Envelope, FileStorage, SystemClock, default_data_dir, spawn,
};
use ampghost_midi::MidirBackend;
use anyhow::{Context, Result};
use futures_channel::mpsc::{UnboundedReceiver, unbounded};

pub struct Bridge {
    pub handle: EngineHandle,
    pub events: UnboundedReceiver<Envelope>,
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
    let (sender, events) = unbounded();
    let handle = spawn(
        engine,
        Box::new(move |envelope| {
            if sender.unbounded_send(envelope).is_err() {
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
