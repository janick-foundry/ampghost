use std::collections::BTreeMap;
use std::sync::OnceLock;

use tsuba_protocol::{BackupLayout, PatchBlock, PatchImage, PatchLayout, PatchOffset};

use crate::error::StoreError;

const MK2_DEFAULT_BLOCKS_JSON: &str = include_str!("../data/mk2_default_blocks.json");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamedBlock {
    pub name: String,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BlockImport {
    pub defaulted: Vec<String>,
    pub padded: Vec<String>,
    pub skipped: Vec<String>,
    pub unknown: Vec<String>,
}

fn parse_defaults() -> Result<BTreeMap<String, Vec<u8>>, StoreError> {
    let rows: BTreeMap<String, String> = serde_json::from_str(MK2_DEFAULT_BLOCKS_JSON)?;
    rows.into_iter()
        .map(|(name, text)| Ok((name, crate::preset::unhex(&text)?)))
        .collect()
}

pub fn default_block(block: &PatchBlock) -> Result<Vec<u8>, StoreError> {
    static DEFAULTS: OnceLock<Result<BTreeMap<String, Vec<u8>>, String>> = OnceLock::new();
    let defaults = DEFAULTS
        .get_or_init(|| parse_defaults().map_err(|error| error.to_string()))
        .as_ref()
        .map_err(|error| StoreError::Defaults(error.clone()))?;
    let data = defaults
        .get(block.name)
        .ok_or_else(|| StoreError::Defaults(format!("no default for block {}", block.name)))?;
    if data.len() != block.len() {
        return Err(StoreError::Defaults(format!(
            "default for block {} has {} bytes, expected {}",
            block.name,
            data.len(),
            block.size
        )));
    }
    Ok(data.clone())
}

pub fn image_to_blocks(
    image: &PatchImage,
    backup: &BackupLayout,
) -> Result<Vec<NamedBlock>, StoreError> {
    backup
        .patch_blocks
        .iter()
        .map(|block| {
            let data = block
                .span()
                .offsets()
                .map(|offset| image.get(offset))
                .collect::<Option<Vec<u8>>>()
                .ok_or(StoreError::BlockOutsideImage(block.name.to_owned()))?;
            Ok(NamedBlock {
                name: block.name.to_owned(),
                data,
            })
        })
        .collect()
}

pub fn blocks_to_image(
    found: &[NamedBlock],
    layout: &'static PatchLayout,
    backup: &BackupLayout,
) -> Result<(PatchImage, BlockImport), StoreError> {
    let mut image = PatchImage::from_data(layout, vec![0; layout.data_len()])?;
    let mut report = BlockImport::default();
    for entry in found {
        if backup.patch_block(&entry.name).is_none() {
            report.unknown.push(entry.name.clone());
        }
    }
    for block in backup.patch_blocks {
        let mut data = default_block(block)?;
        let supplied = found.iter().find(|entry| entry.name == block.name);
        match supplied {
            None => report.defaulted.push(block.name.to_owned()),
            Some(_) if block.name == backup.status_block => {
                report.skipped.push(block.name.to_owned());
            }
            Some(entry) => {
                if entry.data.len() > data.len() {
                    return Err(StoreError::BlockLength {
                        block: block.name.to_owned(),
                        expected: data.len(),
                        actual: entry.data.len(),
                    });
                }
                if entry.data.len() < data.len() {
                    report.padded.push(block.name.to_owned());
                }
                for (slot, &value) in data.iter_mut().zip(&entry.data) {
                    *slot = value;
                }
            }
        }
        for (position, value) in (block.offset.linear()..).zip(data) {
            image
                .set(PatchOffset::from_linear(position), value)
                .map_err(|_| StoreError::BlockOutsideImage(block.name.to_owned()))?;
        }
    }
    Ok((image, report))
}

pub(crate) fn hex_list(bytes: &[u8]) -> Vec<String> {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

pub(crate) fn unhex_list(block: &str, values: &[String]) -> Result<Vec<u8>, StoreError> {
    values
        .iter()
        .map(|value| {
            let valid = value.len() == 2 && value.chars().all(|c| c.is_ascii_hexdigit());
            let byte = u8::from_str_radix(value, 16)
                .ok()
                .filter(|byte| valid && *byte < 0x80);
            byte.ok_or_else(|| StoreError::BlockValue {
                block: block.to_owned(),
                value: value.clone(),
            })
        })
        .collect()
}
