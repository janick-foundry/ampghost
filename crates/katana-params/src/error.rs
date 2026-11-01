#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ParamsError {
    #[error("embedded parameter table is invalid: {0}")]
    Parse(String),
    #[error("unknown parameter {0}")]
    Unknown(String),
    #[error("value {value} is not valid for parameter {id}")]
    OutOfRange { id: String, value: u16 },
    #[error("parameter {0} is not part of the patch")]
    NotPatch(String),
    #[error("parameter {0} is fixed by the official editor and cannot be changed")]
    Hidden(String),
    #[error("parameter {id} has unsupported size {size}")]
    Size { id: String, size: u8 },
}
