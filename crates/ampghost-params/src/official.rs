use std::sync::OnceLock;

use ampghost_protocol::{PatchImage, PatchOffset};

use crate::ParamsError;

const OFFICIAL_RANGES_JSON: &str = include_str!("../data/official_ranges.json");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OfficialRange {
    pub offset: PatchOffset,
    pub bytes: u8,
    pub min: u16,
    pub max: u16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Normalised {
    pub offset: PatchOffset,
    pub from: u16,
    pub to: u16,
}

fn parse() -> Result<Vec<OfficialRange>, ParamsError> {
    let rows: Vec<[u16; 5]> = serde_json::from_str(OFFICIAL_RANGES_JSON)
        .map_err(|error| ParamsError::Parse(error.to_string()))?;
    rows.into_iter()
        .map(|[page, byte, bytes, min, max]| {
            let page = u8::try_from(page).map_err(|error| ParamsError::Parse(error.to_string()))?;
            let byte = u8::try_from(byte).map_err(|error| ParamsError::Parse(error.to_string()))?;
            let bytes =
                u8::try_from(bytes).map_err(|error| ParamsError::Parse(error.to_string()))?;
            if !(1..=2).contains(&bytes) || min > max {
                return Err(ParamsError::Parse(format!(
                    "official range at {page:02X} {byte:02X} is malformed"
                )));
            }
            Ok(OfficialRange {
                offset: PatchOffset::new(page, byte),
                bytes,
                min,
                max,
            })
        })
        .collect()
}

pub fn official_ranges() -> Result<&'static [OfficialRange], ParamsError> {
    static RANGES: OnceLock<Result<Vec<OfficialRange>, ParamsError>> = OnceLock::new();
    match RANGES.get_or_init(parse) {
        Ok(ranges) => Ok(ranges),
        Err(error) => Err(error.clone()),
    }
}

fn read(image: &PatchImage, range: &OfficialRange) -> Option<u16> {
    let first = u16::from(image.get(range.offset)?);
    if range.bytes == 1 {
        return Some(first);
    }
    let second = u16::from(image.get(range.offset.plus(1))?);
    Some((first << 7) | second)
}

fn write(image: &mut PatchImage, range: &OfficialRange, value: u16) -> Result<(), ParamsError> {
    let failed = || ParamsError::Parse(format!("cannot set official value at {}", range.offset));
    let low = u8::try_from(value & 0x7F).map_err(|_| failed())?;
    if range.bytes == 1 {
        return image.set(range.offset, low).map_err(|_| failed());
    }
    let high = u8::try_from((value >> 7) & 0x7F).map_err(|_| failed())?;
    image.set(range.offset, high).map_err(|_| failed())?;
    image.set(range.offset.plus(1), low).map_err(|_| failed())
}

pub fn conform_to_official(image: &mut PatchImage) -> Result<Vec<Normalised>, ParamsError> {
    let mut normalised = Vec::new();
    for range in official_ranges()? {
        let Some(value) = read(image, range) else {
            continue;
        };
        if (range.min..=range.max).contains(&value) {
            continue;
        }
        if range.min == range.max {
            write(image, range, range.min)?;
            normalised.push(Normalised {
                offset: range.offset,
                from: value,
                to: range.min,
            });
            continue;
        }
        return Err(ParamsError::Unofficial {
            offset: range.offset.to_string(),
            value,
            min: range.min,
            max: range.max,
        });
    }
    Ok(normalised)
}
