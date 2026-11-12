use std::fmt;
use std::str::FromStr;

use crate::error::ProtocolError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Channel {
    Panel,
    A1,
    A2,
    A3,
    A4,
    B1,
    B2,
    B3,
    B4,
}

impl Channel {
    pub const ALL: [Self; 9] = [
        Self::Panel,
        Self::A1,
        Self::A2,
        Self::A3,
        Self::A4,
        Self::B1,
        Self::B2,
        Self::B3,
        Self::B4,
    ];

    pub const fn index(self) -> u8 {
        match self {
            Self::Panel => 0,
            Self::A1 => 1,
            Self::A2 => 2,
            Self::A3 => 3,
            Self::A4 => 4,
            Self::B1 => 5,
            Self::B2 => 6,
            Self::B3 => 7,
            Self::B4 => 8,
        }
    }

    pub fn from_index(index: u8) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|channel| channel.index() == index)
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Panel => "Panel",
            Self::A1 => "A1",
            Self::A2 => "A2",
            Self::A3 => "A3",
            Self::A4 => "A4",
            Self::B1 => "B1",
            Self::B2 => "B2",
            Self::B3 => "B3",
            Self::B4 => "B4",
        }
    }
}

impl fmt::Display for Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

impl FromStr for Channel {
    type Err = ProtocolError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        Self::ALL
            .into_iter()
            .find(|channel| channel.label().eq_ignore_ascii_case(text.trim()))
            .ok_or_else(|| ProtocolError::UnknownChannel(text.to_owned()))
    }
}

pub fn parse_current_channel(data: &[u8]) -> Result<Channel, ProtocolError> {
    match data {
        [0, index] => Channel::from_index(*index).ok_or(ProtocolError::ChannelReply),
        _ => Err(ProtocolError::ChannelReply),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProgramChangeMap([u8; 9]);

impl ProgramChangeMap {
    pub fn parse(data: &[u8]) -> Result<Self, ProtocolError> {
        let programs: [u8; 9] = data
            .try_into()
            .map_err(|_| ProtocolError::ProgramChangeMap(data.len()))?;
        crate::address::ensure_seven_bit(&programs)?;
        Ok(Self(programs))
    }

    pub fn program(&self, channel: Channel) -> u8 {
        self.0
            .get(usize::from(channel.index()))
            .copied()
            .unwrap_or_default()
    }

    pub fn entries(&self) -> impl Iterator<Item = (Channel, u8)> + '_ {
        Channel::ALL
            .into_iter()
            .map(move |channel| (channel, self.program(channel)))
    }
}
