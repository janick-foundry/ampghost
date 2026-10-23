#![forbid(unsafe_code)]

mod error;
mod param;
mod table;

pub use error::ParamsError;
pub use param::{Area, EnumOption, Param, ParamKind, Status};
pub use table::{ParamTable, SOURCE_ATTRIBUTION, table};
