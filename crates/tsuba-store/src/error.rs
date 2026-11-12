use tsuba_protocol::ProtocolError;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StoreError {
    #[error("preset JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported preset format {format} version {version}")]
    Format { format: String, version: u32 },
    #[error("invalid hex data: {0}")]
    Hex(String),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error("invalid storage key {0:?}")]
    InvalidKey(String),
    #[error("{0} not found in storage")]
    NotFound(String),
    #[error("storage failure: {0}")]
    Storage(String),
    #[error("{0} already exists in storage")]
    AlreadyExists(String),
    #[error("block {block} has {actual} bytes, at most {expected} are allowed")]
    BlockLength {
        block: String,
        expected: usize,
        actual: usize,
    },
    #[error("block {block} holds {value:?}, which is not a 7-bit hex byte")]
    BlockValue { block: String, value: String },
    #[error("block {0} lies outside the patch image")]
    BlockOutsideImage(String),
    #[error("embedded default blocks are invalid: {0}")]
    Defaults(String),
    #[error("device {0:?} is not supported, expected \"KATANA MkII\"")]
    Device(String),
    #[error("format revision {0:?} is not supported")]
    FormatRev(String),
    #[error("file holds no patches")]
    NoPatches,
    #[error("file holds {0} patches, at most {1} are allowed")]
    TooManyPatches(usize, usize),
    #[error("backup row count {0} does not match the expected {1}")]
    RowCount(usize, usize),
    #[error("preset name must be 1 to 64 printable characters")]
    InvalidName,
}
