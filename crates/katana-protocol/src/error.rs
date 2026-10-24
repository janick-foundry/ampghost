use crate::address::Address;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ProtocolError {
    #[error("byte {value:#04x} at index {index} is not a 7-bit value")]
    NotSevenBit { index: usize, value: u8 },
    #[error("address needs exactly 4 bytes, got {0}")]
    AddressLength(usize),
    #[error("linear value {0} does not fit into 4 x 7 bits")]
    LinearOverflow(u32),
    #[error("message is not a complete SysEx frame")]
    BadFrame,
    #[error("message is not a Roland Katana SysEx message")]
    NotKatana,
    #[error("checksum mismatch at {address}: expected {expected:#04x}, got {actual:#04x}")]
    Checksum {
        address: Address,
        expected: u8,
        actual: u8,
    },
    #[error("DT1 carries no data")]
    EmptyData,
    #[error("not an identity reply")]
    NotIdentity,
    #[error("patch image needs {expected} data bytes, got {actual}")]
    ImageSize { expected: usize, actual: usize },
    #[error("patch offset {0} is not part of the patch layout")]
    OutsideLayout(u32),
    #[error("patch image is incomplete, {missing} bytes missing")]
    IncompleteImage { missing: usize },
    #[error("unknown channel {0}")]
    UnknownChannel(String),
    #[error("channel reply has unexpected shape")]
    ChannelReply,
    #[error("program change map needs 9 bytes, got {0}")]
    ProgramChangeMap(usize),
    #[error("patch name {0:?} must be at most 16 printable ASCII characters")]
    InvalidName(String),
    #[error("write of {len} bytes at {address} is forbidden by the device profile")]
    WriteForbidden { address: Address, len: usize },
    #[error("effect chain must name each of the {expected} blocks exactly once, got [{}]", hex(.order))]
    InvalidChain { expected: usize, order: Vec<u8> },
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}
