#![forbid(unsafe_code)]

mod error;
mod preset;
mod storage;
mod syx;

pub use error::StoreError;
pub use preset::{
    PRESET_FORMAT, PRESET_VERSION, Preset, PresetSummary, decode_preset, encode_preset,
    read_preset_summary,
};
pub use storage::{PresetStorage, preset_key, validate_key};
pub use syx::{decode_syx, encode_syx};
