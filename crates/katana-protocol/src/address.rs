use std::fmt;

use crate::error::ProtocolError;

const SEVEN_BIT_LIMIT: u32 = 1 << 28;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Address([u8; 4]);

impl Address {
    pub const fn from_parts(a: u8, b: u8, c: u8, d: u8) -> Self {
        Self([a & 0x7F, b & 0x7F, c & 0x7F, d & 0x7F])
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let array: [u8; 4] = bytes
            .try_into()
            .map_err(|_| ProtocolError::AddressLength(bytes.len()))?;
        ensure_seven_bit(&array)?;
        Ok(Self(array))
    }

    pub fn from_linear(value: u32) -> Result<Self, ProtocolError> {
        Ok(Self(split_linear(value)?))
    }

    pub const fn bytes(self) -> [u8; 4] {
        self.0
    }

    pub const fn linear(self) -> u32 {
        join_linear(self.0)
    }

    pub fn offset(self, distance: u32) -> Result<Self, ProtocolError> {
        let value = self
            .linear()
            .checked_add(distance)
            .ok_or(ProtocolError::LinearOverflow(u32::MAX))?;
        Self::from_linear(value)
    }

    pub fn distance_from(self, base: Self) -> Option<u32> {
        self.linear().checked_sub(base.linear())
    }
}

impl fmt::Display for Address {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex4(f, self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Size(u32);

impl Size {
    pub fn from_linear(value: u32) -> Result<Self, ProtocolError> {
        if value >= SEVEN_BIT_LIMIT {
            return Err(ProtocolError::LinearOverflow(value));
        }
        Ok(Self(value))
    }

    pub const fn from_parts(a: u8, b: u8, c: u8, d: u8) -> Self {
        Self(join_linear([a & 0x7F, b & 0x7F, c & 0x7F, d & 0x7F]))
    }

    pub const fn linear(self) -> u32 {
        self.0
    }

    pub fn bytes(self) -> [u8; 4] {
        split_linear(self.0).unwrap_or([0x7F; 4])
    }
}

impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_hex4(f, self.bytes())
    }
}

fn write_hex4(out: &mut fmt::Formatter<'_>, bytes: [u8; 4]) -> fmt::Result {
    let [first, second, third, fourth] = bytes;
    write!(out, "{first:02X} {second:02X} {third:02X} {fourth:02X}")
}

pub(crate) fn ensure_seven_bit(bytes: &[u8]) -> Result<(), ProtocolError> {
    match bytes.iter().position(|&value| value > 0x7F) {
        Some(index) => Err(ProtocolError::NotSevenBit {
            index,
            value: bytes.get(index).copied().unwrap_or_default(),
        }),
        None => Ok(()),
    }
}

const fn join_linear(bytes: [u8; 4]) -> u32 {
    ((bytes[0] as u32) << 21)
        | ((bytes[1] as u32) << 14)
        | ((bytes[2] as u32) << 7)
        | bytes[3] as u32
}

fn split_linear(value: u32) -> Result<[u8; 4], ProtocolError> {
    if value >= SEVEN_BIT_LIMIT {
        return Err(ProtocolError::LinearOverflow(value));
    }
    let byte = |shift: u32| u8::try_from((value >> shift) & 0x7F).unwrap_or_default();
    Ok([byte(21), byte(14), byte(7), byte(0)])
}
