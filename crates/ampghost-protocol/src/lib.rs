#![forbid(unsafe_code)]

mod address;
mod blocks;
mod channel;
mod error;
mod identity;
mod patch;
mod plan;
mod profile;
mod sysex;
mod verify;

pub use address::{Address, Size};
pub use blocks::{BackupLayout, MK2_PATCH_BLOCKS, MK2_SYSTEM_BLOCKS, PatchBlock, SystemBlock};
pub use channel::{Channel, ProgramChangeMap, parse_current_channel};
pub use error::ProtocolError;
pub use identity::{IDENTITY_REQUEST, Identity};
pub use patch::{PatchAssembler, PatchImage, PatchLayout, PatchOffset, Span};
pub use plan::{WriteOrder, plan_full_write, plan_partial_write};
pub use profile::{ChannelRow, DeviceProfile, KATANA_50_MK2, PROFILES, profile_for};
pub use sysex::{Dt1, Incoming, Rq1, checksum, decode, program_change, split_messages};
pub use verify::{ByteMismatch, LinkedType, VerifyReport, VerifyRules, verify};
