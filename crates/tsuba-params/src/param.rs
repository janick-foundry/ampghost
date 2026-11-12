use serde::Deserialize;
use tsuba_protocol::PatchOffset;

use crate::error::ParamsError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Area {
    Patch,
    System,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    Research,
    Verified { date: &'static str },
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct EnumOption {
    #[serde(rename = "v")]
    pub value: u16,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum ParamKind {
    Range {
        raw_min: u16,
        raw_max: u16,
        disp_min: f64,
        disp_max: f64,
        #[serde(default)]
        unit: String,
        #[serde(default)]
        off_value: Option<u16>,
    },
    Enum {
        options: Vec<EnumOption>,
    },
    Table {
        raw_min: u16,
        raw_max: u16,
        labels: Vec<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub(crate) struct RawParam {
    pub(crate) id: String,
    pub(crate) area: Area,
    pub(crate) addr: [u8; 4],
    pub(crate) block: String,
    #[serde(default)]
    pub(crate) sub: Option<String>,
    pub(crate) label: String,
    pub(crate) size: u8,
    #[serde(flatten)]
    pub(crate) kind: ParamKind,
    #[serde(default)]
    pub(crate) hidden: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    raw: RawParam,
    status: Status,
}

impl Param {
    pub(crate) fn new(raw: RawParam, status: Status) -> Self {
        Self { raw, status }
    }

    pub fn id(&self) -> &str {
        &self.raw.id
    }

    pub fn area(&self) -> Area {
        self.raw.area
    }

    pub fn address_bytes(&self) -> [u8; 4] {
        self.raw.addr
    }

    pub fn block(&self) -> &str {
        &self.raw.block
    }

    pub fn sub(&self) -> Option<&str> {
        self.raw.sub.as_deref()
    }

    pub fn label(&self) -> &str {
        &self.raw.label
    }

    pub fn size(&self) -> u8 {
        self.raw.size
    }

    pub fn kind(&self) -> &ParamKind {
        &self.raw.kind
    }

    pub fn is_hidden(&self) -> bool {
        self.raw.hidden
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    pub fn patch_offset(&self) -> Result<PatchOffset, ParamsError> {
        match self.raw.area {
            Area::Patch => {
                let [_, _, page, byte] = self.raw.addr;
                Ok(PatchOffset::new(page, byte))
            }
            Area::System => Err(ParamsError::NotPatch(self.raw.id.clone())),
        }
    }

    pub fn accepts(&self, value: u16) -> bool {
        match &self.raw.kind {
            ParamKind::Range {
                raw_min, raw_max, ..
            }
            | ParamKind::Table {
                raw_min, raw_max, ..
            } => (*raw_min..=*raw_max).contains(&value),
            ParamKind::Enum { options } => options.iter().any(|option| option.value == value),
        }
    }

    pub fn encode(&self, value: u16) -> Result<Vec<u8>, ParamsError> {
        if !self.accepts(value) {
            return Err(ParamsError::OutOfRange {
                id: self.raw.id.clone(),
                value,
            });
        }
        let low = u8::try_from(value & 0x7F).unwrap_or_default();
        let high = u8::try_from((value >> 7) & 0x7F).unwrap_or_default();
        match self.raw.size {
            1 if value <= 0x7F => Ok(vec![low]),
            1 => Err(ParamsError::OutOfRange {
                id: self.raw.id.clone(),
                value,
            }),
            2 => Ok(vec![high, low]),
            size => Err(ParamsError::Size {
                id: self.raw.id.clone(),
                size,
            }),
        }
    }

    pub fn decode(&self, bytes: &[u8]) -> Result<u16, ParamsError> {
        match (self.raw.size, bytes) {
            (1, [value]) => Ok(u16::from(*value)),
            (2, [high, low]) => Ok((u16::from(*high) << 7) | u16::from(*low)),
            (size, _) => Err(ParamsError::Size {
                id: self.raw.id.clone(),
                size,
            }),
        }
    }

    pub fn describe(&self, value: u16) -> String {
        match &self.raw.kind {
            ParamKind::Range {
                raw_min,
                raw_max,
                disp_min,
                disp_max,
                unit,
                off_value,
            } => {
                if *off_value == Some(value) {
                    return "Off".to_owned();
                }
                let span = f64::from(raw_max.saturating_sub(*raw_min)).max(1.0);
                let shown = disp_min
                    + f64::from(value.saturating_sub(*raw_min)) * (disp_max - disp_min) / span;
                let decimals = usize::from(disp_min.fract() != 0.0 || disp_max.fract() != 0.0);
                format!(
                    "{shown:.decimals$}{}{unit}",
                    if unit.is_empty() { "" } else { " " }
                )
            }
            ParamKind::Enum { options } => options
                .iter()
                .find(|option| option.value == value)
                .map_or_else(|| format!("raw {value}"), |option| option.label.clone()),
            ParamKind::Table {
                raw_min, labels, ..
            } => labels
                .get(usize::from(value.saturating_sub(*raw_min)))
                .cloned()
                .unwrap_or_else(|| format!("raw {value}")),
        }
    }
}
