use std::fmt;

use crate::address::{Address, Size, ensure_seven_bit};
use crate::error::ProtocolError;
use crate::identity::Identity;

const SYSEX_START: u8 = 0xF0;
const SYSEX_END: u8 = 0xF7;
const ROLAND_HEADER: [u8; 7] = [SYSEX_START, 0x41, 0x00, 0x00, 0x00, 0x00, 0x33];
const COMMAND_RQ1: u8 = 0x11;
const COMMAND_DT1: u8 = 0x12;
const PROGRAM_CHANGE_CHANNEL_1: u8 = 0xC0;
const FRAME_OVERHEAD: usize = ROLAND_HEADER.len() + 1 + 4 + 2;

pub fn checksum(bytes: &[u8]) -> u8 {
    let sum = bytes.iter().fold(0_u8, |acc, &byte| acc.wrapping_add(byte));
    0_u8.wrapping_sub(sum) & 0x7F
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rq1 {
    address: Address,
    size: Size,
}

impl Rq1 {
    pub const fn new(address: Address, size: Size) -> Self {
        Self { address, size }
    }

    pub const fn address(&self) -> Address {
        self.address
    }

    pub const fn size(&self) -> Size {
        self.size
    }

    pub fn end(&self) -> u32 {
        self.address.linear().saturating_add(self.size.linear())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(8);
        body.extend_from_slice(&self.address.bytes());
        body.extend_from_slice(&self.size.bytes());
        frame(COMMAND_RQ1, &body)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dt1 {
    address: Address,
    data: Vec<u8>,
}

impl Dt1 {
    pub fn new(address: Address, data: impl Into<Vec<u8>>) -> Result<Self, ProtocolError> {
        let data = data.into();
        if data.is_empty() {
            return Err(ProtocolError::EmptyData);
        }
        ensure_seven_bit(&data)?;
        Ok(Self { address, data })
    }

    pub const fn address(&self) -> Address {
        self.address
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn start(&self) -> u32 {
        self.address.linear()
    }

    pub fn end(&self) -> u32 {
        let len = u32::try_from(self.data.len()).unwrap_or(u32::MAX);
        self.start().saturating_add(len)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(4 + self.data.len());
        body.extend_from_slice(&self.address.bytes());
        body.extend_from_slice(&self.data);
        frame(COMMAND_DT1, &body)
    }
}

impl fmt::Display for Dt1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "DT1 {} [{} B]", self.address, self.data.len())?;
        if self.data.len() <= 4 {
            for byte in &self.data {
                write!(f, " {byte:02X}")?;
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Incoming {
    Dt1(Dt1),
    Identity(Identity),
    Other(Vec<u8>),
}

pub fn decode(message: &[u8]) -> Result<Incoming, ProtocolError> {
    if message.first() != Some(&SYSEX_START) {
        return Ok(Incoming::Other(message.to_vec()));
    }
    if message.last() != Some(&SYSEX_END) {
        return Err(ProtocolError::BadFrame);
    }
    if let Ok(identity) = Identity::parse(message) {
        return Ok(Incoming::Identity(identity));
    }
    if !message.starts_with(&ROLAND_HEADER) {
        return Ok(Incoming::Other(message.to_vec()));
    }
    if message.len() < FRAME_OVERHEAD + 1 {
        return Err(ProtocolError::BadFrame);
    }
    let command = message.get(ROLAND_HEADER.len()).copied();
    if command != Some(COMMAND_DT1) {
        return Ok(Incoming::Other(message.to_vec()));
    }
    let body_end = message.len() - 2;
    let body = message
        .get(ROLAND_HEADER.len() + 1..body_end)
        .ok_or(ProtocolError::BadFrame)?;
    let received = message.get(body_end).copied().unwrap_or_default();
    let (address_bytes, data) = body.split_at_checked(4).ok_or(ProtocolError::BadFrame)?;
    let address = Address::from_bytes(address_bytes)?;
    let expected = checksum(body);
    if expected != received {
        return Err(ProtocolError::Checksum {
            address,
            expected,
            actual: received,
        });
    }
    Dt1::new(address, data).map(Incoming::Dt1)
}

pub fn split_messages(stream: &[u8]) -> Vec<&[u8]> {
    let mut messages = Vec::new();
    let mut rest = stream;
    while let Some(start) = rest.iter().position(|&byte| byte == SYSEX_START) {
        let Some(tail) = rest.get(start..) else {
            break;
        };
        let Some(end) = tail.iter().position(|&byte| byte == SYSEX_END) else {
            break;
        };
        if let Some(message) = tail.get(..=end) {
            messages.push(message);
        }
        rest = tail.get(end + 1..).unwrap_or_default();
    }
    messages
}

pub fn program_change(program: u8) -> [u8; 2] {
    [PROGRAM_CHANGE_CHANNEL_1, program & 0x7F]
}

fn frame(command: u8, body: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(FRAME_OVERHEAD + body.len());
    out.extend_from_slice(&ROLAND_HEADER);
    out.push(command);
    out.extend_from_slice(body);
    out.push(checksum(body));
    out.push(SYSEX_END);
    out
}
