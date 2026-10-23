use std::fmt;

use crate::address::{Address, Size};
use crate::error::ProtocolError;
use crate::sysex::Dt1;

const NAME_LEN: u32 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PatchOffset(u32);

impl PatchOffset {
    pub const fn new(page: u8, byte: u8) -> Self {
        Self((((page & 0x7F) as u32) << 7) | (byte & 0x7F) as u32)
    }

    pub const fn from_linear(value: u32) -> Self {
        Self(value)
    }

    pub const fn linear(self) -> u32 {
        self.0
    }

    pub const fn page(self) -> u32 {
        self.0 >> 7
    }

    pub const fn byte(self) -> u32 {
        self.0 & 0x7F
    }

    pub const fn plus(self, distance: u32) -> Self {
        Self(self.0 + distance)
    }

    pub fn address(self, base: Address) -> Result<Address, ProtocolError> {
        base.offset(self.0)
    }
}

impl fmt::Display for PatchOffset {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:02X} {:02X}", self.page(), self.byte())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    start: PatchOffset,
    len: u32,
}

impl Span {
    pub const fn new(start: PatchOffset, len: u32) -> Self {
        Self { start, len }
    }

    pub const fn between(start: PatchOffset, last: PatchOffset) -> Self {
        Self {
            start,
            len: last.linear() - start.linear() + 1,
        }
    }

    pub const fn start(&self) -> PatchOffset {
        self.start
    }

    pub const fn len(&self) -> u32 {
        self.len
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub const fn end(&self) -> u32 {
        self.start.linear() + self.len
    }

    pub const fn contains(&self, offset: PatchOffset) -> bool {
        offset.linear() >= self.start.linear() && offset.linear() < self.end()
    }

    pub fn offsets(&self) -> impl Iterator<Item = PatchOffset> {
        (self.start.linear()..self.end()).map(PatchOffset::from_linear)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct PatchLayout {
    size: Size,
    returned: &'static [Span],
    reply_packet_len: u32,
}

impl PatchLayout {
    pub const fn new(size: Size, returned: &'static [Span], reply_packet_len: u32) -> Self {
        Self {
            size,
            returned,
            reply_packet_len,
        }
    }

    pub const fn size(&self) -> Size {
        self.size
    }

    pub const fn returned(&self) -> &'static [Span] {
        self.returned
    }

    pub fn data_len(&self) -> usize {
        self.returned
            .iter()
            .map(|span| usize::try_from(span.len()).unwrap_or_default())
            .sum()
    }

    pub fn index_of(&self, offset: PatchOffset) -> Option<usize> {
        let mut before = 0_u32;
        for span in self.returned {
            if span.contains(offset) {
                let index = before + (offset.linear() - span.start().linear());
                return usize::try_from(index).ok();
            }
            before += span.len();
        }
        None
    }

    pub fn contains(&self, offset: PatchOffset) -> bool {
        self.returned.iter().any(|span| span.contains(offset))
    }

    pub fn offsets(&self) -> impl Iterator<Item = PatchOffset> + '_ {
        self.returned.iter().flat_map(Span::offsets)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PatchImage {
    layout: &'static PatchLayout,
    data: Vec<u8>,
}

impl PatchImage {
    pub fn from_data(layout: &'static PatchLayout, data: Vec<u8>) -> Result<Self, ProtocolError> {
        if data.len() != layout.data_len() {
            return Err(ProtocolError::ImageSize {
                expected: layout.data_len(),
                actual: data.len(),
            });
        }
        crate::address::ensure_seven_bit(&data)?;
        Ok(Self { layout, data })
    }

    pub fn from_packets(
        layout: &'static PatchLayout,
        base: Address,
        packets: &[Dt1],
    ) -> Result<Self, ProtocolError> {
        let mut data = vec![0_u8; layout.data_len()];
        let mut filled = vec![false; layout.data_len()];
        for packet in packets {
            let Some(start) = packet.address().distance_from(base) else {
                continue;
            };
            for (position, &value) in (start..).zip(packet.data()) {
                if let Some(index) = layout.index_of(PatchOffset::from_linear(position))
                    && let (Some(slot), Some(flag)) = (data.get_mut(index), filled.get_mut(index))
                {
                    *slot = value;
                    *flag = true;
                }
            }
        }
        let missing = filled.iter().filter(|flag| !**flag).count();
        if missing > 0 {
            return Err(ProtocolError::IncompleteImage { missing });
        }
        Ok(Self { layout, data })
    }

    pub const fn layout(&self) -> &'static PatchLayout {
        self.layout
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn get(&self, offset: PatchOffset) -> Option<u8> {
        self.layout
            .index_of(offset)
            .and_then(|index| self.data.get(index).copied())
    }

    pub fn set(&mut self, offset: PatchOffset, value: u8) -> Result<(), ProtocolError> {
        if value > 0x7F {
            return Err(ProtocolError::NotSevenBit { index: 0, value });
        }
        let slot = self
            .layout
            .index_of(offset)
            .and_then(|index| self.data.get_mut(index))
            .ok_or(ProtocolError::OutsideLayout(offset.linear()))?;
        *slot = value;
        Ok(())
    }

    pub fn name(&self) -> String {
        (0..NAME_LEN)
            .filter_map(|position| self.get(PatchOffset::from_linear(position)))
            .map(char::from)
            .collect::<String>()
            .trim_end()
            .to_owned()
    }

    pub fn to_packets(&self, base: Address) -> Result<Vec<Dt1>, ProtocolError> {
        let mut packets = Vec::new();
        for span in self.layout.returned() {
            let mut position = span.start().linear();
            while position < span.end() {
                let len = (span.end() - position).min(self.layout.reply_packet_len);
                let bytes = (position..position + len)
                    .filter_map(|value| self.get(PatchOffset::from_linear(value)))
                    .collect::<Vec<_>>();
                packets.push(Dt1::new(base.offset(position)?, bytes)?);
                position += len;
            }
        }
        Ok(packets)
    }

    pub fn differing_offsets(&self, other: &Self) -> Vec<PatchOffset> {
        self.layout
            .offsets()
            .filter(|&offset| self.get(offset) != other.get(offset))
            .collect()
    }
}
