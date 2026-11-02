use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::blocks::NamedBlock;
use crate::error::StoreError;
use crate::ordered::{OrderedBlocks, named_blocks};

pub const ALB_FORMAT_REV: &str = "0003";
pub const ALB_DEVICE: &str = "KATANA MkII";
pub const ALB_MODEL: &str = "KATANA Mk II";
pub const ALB_ROWS: usize = 9;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FullBackup {
    pub system: Vec<NamedBlock>,
    pub user_patches: Vec<Option<Vec<NamedBlock>>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlbFile {
    pub format_rev: String,
    pub device: String,
    pub model: Option<String>,
    pub backup: FullBackup,
}

#[derive(Serialize)]
struct AlbOut<'a> {
    system: OrderedBlocks<'a>,
    #[serde(rename = "userPatch")]
    user_patch: Vec<Option<OrderedBlocks<'a>>>,
    #[serde(rename = "formatRev")]
    format_rev: &'static str,
    device: &'static str,
    model: &'static str,
}

#[derive(Deserialize)]
struct AlbIn {
    system: BTreeMap<String, Vec<String>>,
    #[serde(rename = "userPatch")]
    user_patch: Vec<Option<BTreeMap<String, Vec<String>>>>,
    #[serde(rename = "formatRev")]
    format_rev: String,
    device: String,
    model: Option<String>,
}

pub fn encode_alb(backup: &FullBackup) -> Result<String, StoreError> {
    if backup.user_patches.len() != ALB_ROWS {
        return Err(StoreError::RowCount(backup.user_patches.len(), ALB_ROWS));
    }
    let file = AlbOut {
        system: OrderedBlocks {
            prefix: "System",
            blocks: &backup.system,
        },
        user_patch: backup
            .user_patches
            .iter()
            .map(|row| {
                row.as_deref().map(|blocks| OrderedBlocks {
                    prefix: "UserPatch",
                    blocks,
                })
            })
            .collect(),
        format_rev: ALB_FORMAT_REV,
        device: ALB_DEVICE,
        model: ALB_MODEL,
    };
    Ok(serde_json::to_string(&file)?)
}

pub fn decode_alb(text: &str) -> Result<AlbFile, StoreError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let file: AlbIn = serde_json::from_str(text)?;
    if file.device != ALB_DEVICE {
        return Err(StoreError::Device(file.device));
    }
    let user_patches = file
        .user_patch
        .into_iter()
        .map(|row| {
            row.map(|entries| named_blocks("UserPatch", entries))
                .transpose()
        })
        .collect::<Result<Vec<_>, StoreError>>()?;
    Ok(AlbFile {
        format_rev: file.format_rev,
        device: file.device,
        model: file.model,
        backup: FullBackup {
            system: named_blocks("System", file.system)?,
            user_patches,
        },
    })
}
