use katana_protocol::ProtocolError;

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
    #[error("preset name must be 1 to 64 printable characters")]
    InvalidName,
}
