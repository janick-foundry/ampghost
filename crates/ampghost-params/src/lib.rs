#![forbid(unsafe_code)]

mod error;
mod official;
mod param;
mod table;

pub use error::ParamsError;
pub use official::{Normalised, OfficialRange, conform_to_official, official_ranges};
pub use param::{Area, EnumOption, Param, ParamKind, Status};
pub use table::{ParamTable, SOURCE_ATTRIBUTION, table};
