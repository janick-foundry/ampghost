use std::fmt;

use crate::error::ProtocolError;

pub const IDENTITY_REQUEST: [u8; 6] = [0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7];

const REPLY_LEN: usize = 15;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Identity {
    device_id: u8,
    signature: [u8; 9],
}

impl Identity {
    pub fn parse(message: &[u8]) -> Result<Self, ProtocolError> {
        let shape_ok = message.len() == REPLY_LEN
            && message.first() == Some(&0xF0)
            && message.get(1) == Some(&0x7E)
            && message.get(3) == Some(&0x06)
            && message.get(4) == Some(&0x02)
            && message.last() == Some(&0xF7);
        if !shape_ok {
            return Err(ProtocolError::NotIdentity);
        }
        let device_id = message.get(2).copied().ok_or(ProtocolError::NotIdentity)?;
        let signature: [u8; 9] = message
            .get(5..14)
            .and_then(|bytes| bytes.try_into().ok())
            .ok_or(ProtocolError::NotIdentity)?;
        Ok(Self {
            device_id,
            signature,
        })
    }

    pub const fn device_id(&self) -> u8 {
        self.device_id
    }

    pub const fn signature(&self) -> [u8; 9] {
        self.signature
    }

    pub const fn manufacturer(&self) -> u8 {
        self.signature[0]
    }

    pub const fn family(&self) -> [u8; 2] {
        [self.signature[1], self.signature[2]]
    }

    pub const fn member(&self) -> [u8; 2] {
        [self.signature[3], self.signature[4]]
    }

    pub const fn revision(&self) -> [u8; 4] {
        [
            self.signature[5],
            self.signature[6],
            self.signature[7],
            self.signature[8],
        ]
    }

    pub fn hex(&self) -> String {
        hex_bytes(&self.signature)
    }
}

impl fmt::Display for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "device {:02X}, signature {}", self.device_id, self.hex())
    }
}

fn hex_bytes(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}
