use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::blocks::NamedBlock;
use crate::error::StoreError;
use crate::ordered::{OrderedBlocks, named_blocks};

pub const TSL_DEVICE: &str = "KATANA MkII";
pub const TSL_FORMAT_REV: &str = "0002";
pub const TSL_MAX_PATCHES: usize = 15;
const TSL_READABLE_REVS: [&str; 2] = ["0001", "0002"];
const PREFIX: &str = "UserPatch";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSetPatch {
    pub memo: String,
    pub blocks: Vec<NamedBlock>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveSet {
    pub name: String,
    pub format_rev: String,
    pub patches: Vec<LiveSetPatch>,
}

#[derive(Serialize)]
struct TslOut<'a> {
    name: &'a str,
    #[serde(rename = "formatRev")]
    format_rev: &'static str,
    device: &'static str,
    data: [Vec<PatchOut<'a>>; 1],
}

#[derive(Serialize)]
struct PatchOut<'a> {
    memo: &'a str,
    #[serde(rename = "paramSet")]
    param_set: OrderedBlocks<'a>,
}

#[derive(Deserialize)]
struct TslIn {
    #[serde(default)]
    name: String,
    #[serde(rename = "formatRev")]
    format_rev: String,
    device: String,
    data: Vec<Vec<PatchIn>>,
}

#[derive(Deserialize)]
struct PatchIn {
    #[serde(default)]
    memo: String,
    #[serde(rename = "paramSet")]
    param_set: BTreeMap<String, Vec<String>>,
}

pub fn encode_tsl(name: &str, patches: &[LiveSetPatch]) -> Result<String, StoreError> {
    if patches.is_empty() {
        return Err(StoreError::NoPatches);
    }
    if patches.len() > TSL_MAX_PATCHES {
        return Err(StoreError::TooManyPatches(patches.len(), TSL_MAX_PATCHES));
    }
    let file = TslOut {
        name,
        format_rev: TSL_FORMAT_REV,
        device: TSL_DEVICE,
        data: [patches
            .iter()
            .map(|patch| PatchOut {
                memo: &patch.memo,
                param_set: OrderedBlocks {
                    prefix: PREFIX,
                    blocks: &patch.blocks,
                },
            })
            .collect()],
    };
    Ok(serde_json::to_string(&file)?)
}

pub fn decode_tsl(text: &str) -> Result<LiveSet, StoreError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let probe: serde_json::Value = serde_json::from_str(text)?;
    let device = probe
        .get("device")
        .and_then(serde_json::Value::as_str)
        .unwrap_or_default();
    if device != TSL_DEVICE {
        return Err(StoreError::Device(device.to_owned()));
    }
    let file: TslIn = serde_json::from_value(probe)?;
    if file.device != TSL_DEVICE {
        return Err(StoreError::Device(file.device));
    }
    if !TSL_READABLE_REVS.contains(&file.format_rev.as_str()) {
        return Err(StoreError::FormatRev(file.format_rev));
    }
    let patches = file
        .data
        .into_iter()
        .next()
        .unwrap_or_default()
        .into_iter()
        .map(|patch| {
            Ok(LiveSetPatch {
                memo: patch.memo,
                blocks: named_blocks(PREFIX, patch.param_set)?,
            })
        })
        .collect::<Result<Vec<_>, StoreError>>()?;
    if patches.is_empty() {
        return Err(StoreError::NoPatches);
    }
    Ok(LiveSet {
        name: file.name,
        format_rev: file.format_rev,
        patches,
    })
}
