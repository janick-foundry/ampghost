mod build_info;
mod clock;
mod command;
mod engine;
mod error;
#[cfg(feature = "native")]
mod native;
mod session;
mod timings;

#[cfg(feature = "native")]
pub use build_info::build_time;
pub use build_info::{BUILD_COMMIT, VERSION_LABEL};
pub use clock::Clock;
pub use command::{
    AmpSwitch, ChainOutcome, Command, DetectReport, DeviceInfo, Envelope, Event, ParamWrite,
    PresetEntry, RequestId, Stage, SwitchOutcome, WriteOutcome,
};
pub use engine::{Engine, apply_param_writes};
pub use error::EngineError;
#[cfg(feature = "native")]
pub use native::{EngineHandle, FileStorage, SystemClock, default_data_dir, spawn};
pub use timings::Timings;

macro_rules! base_app_id {
    () => {
        "io.github.janick_foundry.AmpGhost"
    };
}

pub const BASE_APP_ID: &str = base_app_id!();
#[cfg(not(feature = "dev"))]
pub const APP_ID: &str = BASE_APP_ID;
#[cfg(feature = "dev")]
pub const APP_ID: &str = concat!(base_app_id!(), ".Devel");
pub const DEV_BUILD: bool = cfg!(feature = "dev");
