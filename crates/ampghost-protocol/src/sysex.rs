use std::fmt;

use crate::address::{Address, Size, ensure_seven_bit};
use crate::error::ProtocolError;
use crate::identity::Identity;

const SYSEX_START: u8 = 0xF0;
const SYSEX_END: u8 = 0xF7;
const ROLAND_ID: u8 = 0x41;
const COMMAND_RQ1: u8 = 0x11;
const COMMAND_DT1: u8 = 0x12;
const PROGRAM_CHANGE_CHANNEL_1: u8 = 0xC0;
const ADDRESS_LEN: usize = 4;
const TRAILER_LEN: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SysexHeader {
    device_id: u8,
    model_id: &'static [u8],
}

impl SysexHeader {
    pub const fn new(device_id: u8, model_id: &'static [u8]) -> Self {
        Self {
            device_id,
            model_id,
        }
    }

    pub const fn device_id(&self) -> u8 {
        self.device_id
    }

    pub const fn model_id(&self) -> &'static [u8] {
        self.model_id
    }

    pub fn bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.len());
        bytes.extend_from_slice(&[SYSEX_START, ROLAND_ID, self.device_id]);
        bytes.extend_from_slice(self.model_id);
        bytes
    }

    const fn len(&self) -> usize {
        3 + self.model_id.len()
    }

    fn matches(&self, message: &[u8]) -> bool {
        message.get(..3) == Some(&[SYSEX_START, ROLAND_ID, self.device_id])
            && message.get(3..self.len()) == Some(self.model_id)
    }

    fn frame_overhead(&self) -> usize {
        self.len() + 1 + ADDRESS_LEN + TRAILER_LEN
    }

    fn frame(&self, command: u8, body: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.frame_overhead() + body.len());
        out.extend_from_slice(&self.bytes());
        out.push(command);
        out.extend_from_slice(body);
        out.push(checksum(body));
        out.push(SYSEX_END);
        out
    }

    pub fn decode(&self, message: &[u8]) -> Result<Incoming, ProtocolError> {
        if message.first() != Some(&SYSEX_START) {
            return Ok(Incoming::Other(message.to_vec()));
        }
        if message.last() != Some(&SYSEX_END) {
            return Err(ProtocolError::BadFrame);
        }
        if let Ok(identity) = Identity::parse(message) {
            return Ok(Incoming::Identity(identity));
        }
        if !self.matches(message) {
            return Ok(Incoming::Other(message.to_vec()));
        }
        if message.len() < self.frame_overhead() + 1 {
            return Err(ProtocolError::BadFrame);
        }
        let command = message.get(self.len()).copied();
        if command != Some(COMMAND_DT1) {
            return Ok(Incoming::Other(message.to_vec()));
        }
        let body_end = message.len() - TRAILER_LEN;
        let body = message
            .get(self.len() + 1..body_end)
            .ok_or(ProtocolError::BadFrame)?;
        let received = message.get(body_end).copied().unwrap_or_default();
        let (address_bytes, data) = body
            .split_at_checked(ADDRESS_LEN)
            .ok_or(ProtocolError::BadFrame)?;
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
}

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

    pub fn encode(&self, header: &SysexHeader) -> Vec<u8> {
        let mut body = Vec::with_capacity(2 * ADDRESS_LEN);
        body.extend_from_slice(&self.address.bytes());
        body.extend_from_slice(&self.size.bytes());
        header.frame(COMMAND_RQ1, &body)
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

    pub fn encode(&self, header: &SysexHeader) -> Vec<u8> {
        let mut body = Vec::with_capacity(ADDRESS_LEN + self.data.len());
        body.extend_from_slice(&self.address.bytes());
        body.extend_from_slice(&self.data);
        header.frame(COMMAND_DT1, &body)
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
