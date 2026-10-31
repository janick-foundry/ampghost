#![forbid(unsafe_code)]

mod address;
mod channel;
mod error;
mod gen3;
mod identity;
mod mk3;
mod patch;
mod plan;
mod profile;
mod sysex;
mod verify;

pub use address::{Address, Size};
pub use channel::{Channel, ProgramChangeMap, parse_current_channel};
pub use error::ProtocolError;
pub use gen3::{
    Gen3Model, Gen3Rq1, KATANA_MK3, decode_gen3_data, gen3_model_for, gen3_read_only_allows,
};
pub use identity::{IDENTITY_REQUEST, Identity};
pub use mk3::{
    MK3_AMP, MK3_CHANNEL, MK3_COLOURS, MK3_KNOBS, MK3_NAME, MK3_SWITCHES, Mk3AmpType, Mk3Channel,
    Mk3Colour, Mk3Effect, Mk3EffectState, Mk3Raw, Mk3State, mk3_delay_time,
};
pub use patch::{PatchAssembler, PatchImage, PatchLayout, PatchOffset, Span};
pub use plan::{WriteOrder, plan_full_write, plan_partial_write};
pub use profile::{DeviceProfile, KATANA_50_MK2, PROFILES, profile_for};
pub use sysex::{Dt1, Incoming, Rq1, checksum, decode, program_change, split_messages};
pub use verify::{ByteMismatch, LinkedType, VerifyReport, VerifyRules, verify};
