#![forbid(unsafe_code)]

mod clock;
mod command;
mod engine;
mod error;
#[cfg(feature = "native")]
mod native;
mod session;
mod timings;

pub use clock::Clock;
pub use command::{
    Command, DetectReport, DeviceInfo, Event, ParamWrite, Stage, SwitchOutcome, WriteOutcome,
};
pub use engine::Engine;
pub use error::EngineError;
#[cfg(feature = "native")]
pub use native::{EngineHandle, FileStorage, SystemClock, default_data_dir, spawn};
pub use timings::Timings;

pub const APP_ID: &str = "io.github.janfoundry.KatanaControl";
