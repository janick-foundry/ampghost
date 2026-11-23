use std::num::TryFromIntError;
use std::sync::Arc;

use ampghost_protocol::ProtocolError;

#[derive(Debug, Clone, thiserror::Error)]
#[non_exhaustive]
pub enum ParamsError {
    #[error("embedded parameter table is invalid: {0}")]
    Parse(String),
    #[error("embedded parameter table is not valid JSON")]
    Json(#[source] Arc<serde_json::Error>),
    #[error("embedded official range holds a value that does not fit")]
    RangeValue(#[from] TryFromIntError),
    #[error("cannot set the official value at {offset}")]
    Conform {
        offset: String,
        #[source]
        source: ProtocolError,
    },
    #[error("unknown parameter {0}")]
    Unknown(String),
    #[error("value {value} is not valid for parameter {id}")]
    OutOfRange { id: String, value: u16 },
    #[error("parameter {0} is not part of the patch")]
    NotPatch(String),
    #[error("parameter {0} is fixed by the official editor and cannot be changed")]
    Hidden(String),
    #[error("value {value} at {offset} is outside the official range {min} to {max}")]
    Unofficial {
        offset: String,
        value: u16,
        min: u16,
        max: u16,
    },
    #[error("parameter {id} has unsupported size {size}")]
    Size { id: String, size: u8 },
}
