use std::fmt;

use crate::address::Address;
use crate::gen3::Gen3Rq1;

pub const MK3_CHANNEL: Gen3Rq1 = Gen3Rq1::new(Address::from_parts(0x00, 0x00, 0x00, 0x00), 2);
pub const MK3_NAME: Gen3Rq1 = Gen3Rq1::new(Address::from_parts(0x20, 0x00, 0x00, 0x00), 16);
pub const MK3_COLOURS: Gen3Rq1 = Gen3Rq1::new(Address::from_parts(0x20, 0x00, 0x04, 0x00), 5);
pub const MK3_AMP: Gen3Rq1 = Gen3Rq1::new(Address::from_parts(0x20, 0x00, 0x06, 0x00), 10);
pub const MK3_SWITCHES: Gen3Rq1 = Gen3Rq1::new(Address::from_parts(0x20, 0x00, 0x08, 0x00), 6);
pub const MK3_KNOBS: Gen3Rq1 = Gen3Rq1::new(Address::from_parts(0x30, 0x00, 0x02, 0x00), 5);

pub fn mk3_delay_time(colour: Mk3Colour) -> Gen3Rq1 {
    let page = match colour {
        Mk3Colour::Green => 0x28,
        Mk3Colour::Red => 0x2A,
        Mk3Colour::Orange => 0x2C,
    };
    Gen3Rq1::new(Address::from_parts(0x20, 0x00, page, 0x01), 4)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mk3Channel {
    Panel,
    Ch1,
    Ch2,
}

impl Mk3Channel {
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Panel),
            1 => Some(Self::Ch1),
            2 => Some(Self::Ch2),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Panel => "Panel",
            Self::Ch1 => "CH1",
            Self::Ch2 => "CH2",
        }
    }
}

impl fmt::Display for Mk3Channel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.label())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mk3Colour {
    Green,
    Red,
    Orange,
}

impl Mk3Colour {
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Green),
            1 => Some(Self::Red),
            2 => Some(Self::Orange),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Green => "Green",
            Self::Red => "Red",
            Self::Orange => "Orange",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mk3AmpType {
    Acoustic,
    Clean,
    Pushed,
    Crunch,
    Lead,
    Brown,
}

impl Mk3AmpType {
    pub fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Acoustic),
            1 => Some(Self::Clean),
            2 => Some(Self::Pushed),
            3 => Some(Self::Crunch),
            4 => Some(Self::Lead),
            5 => Some(Self::Brown),
            _ => None,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Acoustic => "Acoustic",
            Self::Clean => "Clean",
            Self::Pushed => "Pushed",
            Self::Crunch => "Crunch",
            Self::Lead => "Lead",
            Self::Brown => "Brown",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mk3Effect {
    Booster,
    Mod,
    Fx,
    Delay,
    Reverb,
}

impl Mk3Effect {
    pub const ALL: [Self; 5] = [
        Self::Booster,
        Self::Mod,
        Self::Fx,
        Self::Delay,
        Self::Reverb,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Booster => "Booster",
            Self::Mod => "Mod",
            Self::Fx => "FX",
            Self::Delay => "Delay",
            Self::Reverb => "Reverb",
        }
    }

    const fn colour_index(self) -> usize {
        match self {
            Self::Booster => 0,
            Self::Mod => 1,
            Self::Fx => 2,
            Self::Delay => 3,
            Self::Reverb => 4,
        }
    }

    const fn switch_index(self) -> usize {
        match self {
            Self::Booster => 0,
            Self::Mod => 1,
            Self::Fx => 2,
            Self::Delay => 3,
            Self::Reverb => 5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mk3EffectState {
    pub effect: Mk3Effect,
    pub on: bool,
    pub colour: Option<Mk3Colour>,
    pub knob: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Mk3Raw {
    pub channel: u8,
    pub name: Vec<u8>,
    pub colours: Vec<u8>,
    pub amp: Vec<u8>,
    pub switches: Vec<u8>,
    pub knobs: Vec<u8>,
    pub delay_time: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mk3State {
    pub channel: Option<Mk3Channel>,
    pub name: String,
    pub amp_type: Option<Mk3AmpType>,
    pub variation: bool,
    pub gain: u8,
    pub volume: u8,
    pub bass: u8,
    pub middle: u8,
    pub treble: u8,
    pub effects: [Mk3EffectState; 5],
    pub delay_time_ms: u16,
}

impl Mk3State {
    pub fn decode(raw: &Mk3Raw) -> Self {
        let at = |bytes: &[u8], index: usize| bytes.get(index).copied().unwrap_or_default();
        let level = |bytes: &[u8], index: usize| at(bytes, index).min(100);
        let name = raw
            .name
            .iter()
            .map(|&byte| {
                if (0x20..0x7F).contains(&byte) {
                    char::from(byte)
                } else {
                    ' '
                }
            })
            .collect::<String>()
            .trim_end()
            .to_owned();
        let effects = Mk3Effect::ALL.map(|effect| Mk3EffectState {
            effect,
            on: at(&raw.switches, effect.switch_index()) != 0,
            colour: Mk3Colour::from_byte(at(&raw.colours, effect.colour_index())),
            knob: level(&raw.knobs, effect.colour_index()),
        });
        let delay_time_ms = raw
            .delay_time
            .iter()
            .take(4)
            .fold(0_u16, |acc, &nibble| (acc << 4) | u16::from(nibble & 0x0F));
        Self {
            channel: Mk3Channel::from_byte(raw.channel),
            name,
            amp_type: Mk3AmpType::from_byte(at(&raw.amp, 7)),
            variation: at(&raw.amp, 9) != 0,
            gain: level(&raw.amp, 0),
            volume: level(&raw.amp, 1),
            bass: level(&raw.amp, 2),
            middle: level(&raw.amp, 3),
            treble: level(&raw.amp, 4),
            effects,
            delay_time_ms,
        }
    }

    pub fn delay_colour(raw: &Mk3Raw) -> Mk3Colour {
        raw.colours
            .get(Mk3Effect::Delay.colour_index())
            .copied()
            .and_then(Mk3Colour::from_byte)
            .unwrap_or(Mk3Colour::Green)
    }

    pub fn effect(&self, effect: Mk3Effect) -> Option<&Mk3EffectState> {
        self.effects.iter().find(|state| state.effect == effect)
    }
}
