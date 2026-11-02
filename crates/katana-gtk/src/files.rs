use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::Path;

use anyhow::{Context, Result};
use katana_protocol::{DeviceProfile, KATANA_50_MK2, PatchImage};
use katana_store::{FullBackup, blocks_to_image, decode_tsl, encode_alb};

use crate::i18n::{tr, trf};

const PROFILE: &DeviceProfile = &KATANA_50_MK2;

pub struct Tone {
    pub label: String,
    pub loaded: Result<(PatchImage, String), String>,
}

pub struct ToneFile {
    pub name: String,
    pub tones: Vec<Tone>,
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("the chosen path has no file name")?;
    let temporary = path.with_file_name(format!(".{file_name}.{}.part", std::process::id()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .with_context(|| format!("creating {}", temporary.display()))?;
    let written = file
        .write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| std::fs::rename(&temporary, path));
    if let Err(error) = written {
        if let Err(cleanup) = std::fs::remove_file(&temporary) {
            tracing::warn!(%cleanup, "could not remove the partial file");
        }
        return Err(error).with_context(|| format!("writing {}", path.display()));
    }
    Ok(())
}

pub fn save_backup(path: &Path, backup: &FullBackup) -> Result<usize> {
    let text = encode_alb(backup)?;
    write_atomic(path, text.as_bytes())?;
    Ok(backup
        .user_patches
        .iter()
        .filter(|row| row.is_some())
        .count())
}

pub fn load_tones(path: &Path) -> Result<ToneFile> {
    let text =
        std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    let liveset = decode_tsl(&text)?;
    let tones = liveset
        .patches
        .iter()
        .enumerate()
        .map(|(position, patch)| {
            let loaded = load_tone(&patch.blocks);
            let name = loaded
                .as_ref()
                .map(|(image, _)| image.name().trim().to_owned())
                .unwrap_or_default();
            let number = (position + 1).to_string();
            let label = if name.is_empty() {
                trf("{number}. (unnamed)", &[("number", &number)])
            } else {
                format!("{number}. {name}")
            };
            Tone { label, loaded }
        })
        .collect();
    Ok(ToneFile {
        name: liveset.name,
        tones,
    })
}

fn load_tone(blocks: &[katana_store::NamedBlock]) -> Result<(PatchImage, String), String> {
    let (mut image, report) = blocks_to_image(blocks, &PROFILE.patch_layout, &PROFILE.backup)
        .map_err(|error| error.to_string())?;
    let normalised = katana_params::conform_to_official(&mut image).map_err(|error| {
        trf(
            "Outside the official ranges: {error}",
            &[("error", &error.to_string())],
        )
    })?;
    let mut lines = Vec::new();
    let mut list = |label: String, names: &[String]| {
        if !names.is_empty() {
            lines.push(format!("{label} {}", names.join(", ")));
        }
    };
    list(tr("Filled from defaults:"), &report.defaulted);
    list(tr("Padded from defaults:"), &report.padded);
    list(tr("Ignored:"), &report.skipped);
    list(tr("Unknown, ignored:"), &report.unknown);
    if !normalised.is_empty() {
        lines.push(trf(
            "Fixed values set to the official value: {count}",
            &[("count", &normalised.len().to_string())],
        ));
    }
    if lines.is_empty() {
        lines.push(tr("All blocks present."));
    }
    Ok((image, lines.join("\n")))
}
