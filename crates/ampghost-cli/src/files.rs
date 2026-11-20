use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use ampghost_engine::{Clock, FileStorage, SystemClock};
use ampghost_protocol::{DeviceProfile, KATANA_50_MK2, PatchImage};
use ampghost_store::{
    ALB_ROWS, FullBackup, LiveSetPatch, Preset, PresetStorage, TSL_MAX_PATCHES, blocks_to_image,
    decode_alb, decode_preset, decode_tsl, encode_preset, encode_tsl, image_to_blocks, preset_key,
};
use anyhow::{Context, Result, bail};

use crate::cli::Action;
use crate::output::sha256_hex;

const PROFILE: &DeviceProfile = &KATANA_50_MK2;
const DEFAULT_LIVESET_NAME: &str = "Tsuba";

pub fn offline(action: &Action, presets: &FileStorage) -> Option<Result<()>> {
    match action {
        Action::BackupAll {
            plan: true,
            out: None,
            ..
        } => Some(print_backup_plan()),
        Action::BackupAll { plan: true, .. } => Some(Err(anyhow::anyhow!(
            "--plan does not write a file, drop --out"
        ))),
        Action::TslExport {
            from_temp: false,
            out,
            name,
            presets: names,
            replace,
        } => Some(export_presets(
            presets,
            names,
            name.as_deref(),
            out,
            *replace,
        )),
        Action::TslImport {
            to_temp: false,
            file,
            index,
            name,
            replace,
            dry_run,
        } => Some(import_presets(
            presets.clone(),
            file,
            *index,
            name.as_deref(),
            *replace,
            *dry_run,
        )),
        Action::AlbInfo { file } => Some(alb_info(file)),
        _ => None,
    }
}

pub fn key_for(name: &str) -> Result<String> {
    if Path::new(name)
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("json"))
    {
        Ok(name.to_owned())
    } else {
        Ok(preset_key(name)?)
    }
}

pub fn write_new_file(path: &Path, bytes: &[u8], replace: bool) -> Result<()> {
    if !replace && path.exists() {
        bail!(
            "{} already exists; pass --replace to overwrite it",
            path.display()
        );
    }
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .context("output path needs a file name")?;
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
        let _ = std::fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("writing {}", path.display()));
    }
    Ok(())
}

pub fn save_alb(backup: &FullBackup, out: &Path, replace: bool) -> Result<()> {
    let text = ampghost_store::encode_alb(backup)?;
    write_new_file(out, text.as_bytes(), replace)?;
    let rows = backup
        .user_patches
        .iter()
        .enumerate()
        .filter(|(_, row)| row.is_some())
        .map(|(row, _)| row.to_string())
        .collect::<Vec<_>>()
        .join(",");
    println!("backup:     {}", out.display());
    println!("system:     {} blocks", backup.system.len());
    println!("rows:       {rows} of {ALB_ROWS}");
    println!("bytes:      {}", text.len());
    println!("sha256:     {}", sha256_hex(text.as_bytes()));
    Ok(())
}

fn print_backup_plan() -> Result<()> {
    let layout = &PROFILE.backup;
    let mut count = 0;
    let mut payload = 0;
    let mut print = |label: String, address, size| -> Result<()> {
        for request in layout.chunk_requests(address, size)? {
            println!(
                "RQ1 {} {:>4}  {label}",
                request.address(),
                request.size().linear()
            );
            count += 1;
            payload += request.size().linear();
        }
        Ok(())
    };
    for block in layout.system_blocks {
        print(format!("System%{}", block.name), block.address, block.size)?;
    }
    for &row in layout.stored_patch_rows {
        let base = layout.stored_patch_base(row)?;
        for block in layout.patch_blocks {
            print(
                format!("UserPatch({})%{}", row + 1, block.name),
                block.offset.address(base)?,
                block.size,
            )?;
        }
    }
    println!("requests:   {count}");
    println!("payload:    {payload} bytes");
    Ok(())
}

pub fn temp_liveset(image: &PatchImage) -> Result<Vec<LiveSetPatch>> {
    Ok(vec![LiveSetPatch {
        memo: String::new(),
        blocks: image_to_blocks(image, &PROFILE.backup)?,
    }])
}

pub fn save_tsl(name: &str, patches: &[LiveSetPatch], out: &Path, replace: bool) -> Result<()> {
    let text = encode_tsl(name, patches)?;
    write_new_file(out, text.as_bytes(), replace)?;
    println!("liveset:    {} ({name:?})", out.display());
    println!("patches:    {}", patches.len());
    println!("sha256:     {}", sha256_hex(text.as_bytes()));
    Ok(())
}

fn export_presets(
    presets: &FileStorage,
    names: &[String],
    liveset: Option<&str>,
    out: &Path,
    replace: bool,
) -> Result<()> {
    if names.is_empty() {
        bail!("name at least one preset, or pass --from-temp");
    }
    if names.len() > TSL_MAX_PATCHES {
        bail!("a liveset holds at most {TSL_MAX_PATCHES} patches");
    }
    let mut patches = Vec::new();
    let mut first = None;
    for name in names {
        let key = key_for(name)?;
        let text = String::from_utf8(presets.read(&key)?)
            .with_context(|| format!("preset {key} is not UTF-8"))?;
        let preset = decode_preset(&text, &PROFILE.patch_layout, PROFILE.temp_patch)?;
        println!(
            "patch {}:    {} ({key})",
            patches.len(),
            preset.image.name()
        );
        first.get_or_insert_with(|| preset.name.clone());
        patches.push(LiveSetPatch {
            memo: String::new(),
            blocks: image_to_blocks(&preset.image, &PROFILE.backup)?,
        });
    }
    let fallback = if names.len() == 1 {
        first.unwrap_or_else(|| DEFAULT_LIVESET_NAME.to_owned())
    } else {
        DEFAULT_LIVESET_NAME.to_owned()
    };
    save_tsl(liveset.unwrap_or(&fallback), &patches, out, replace)
}

pub fn load_tsl_images(file: &Path, index: Option<usize>) -> Result<Vec<(usize, PatchImage)>> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let liveset = decode_tsl(&text).with_context(|| format!("loading {}", file.display()))?;
    println!(
        "liveset:    {:?} (formatRev {}, {} patches)",
        liveset.name,
        liveset.format_rev,
        liveset.patches.len()
    );
    let selected = match index {
        Some(index) if index >= liveset.patches.len() => {
            bail!(
                "patch {index} does not exist, the file holds {}",
                liveset.patches.len()
            )
        }
        Some(index) => vec![index],
        None => (0..liveset.patches.len()).collect(),
    };
    let mut images = Vec::new();
    for position in selected {
        let Some(patch) = liveset.patches.get(position) else {
            continue;
        };
        let (mut image, report) =
            blocks_to_image(&patch.blocks, &PROFILE.patch_layout, &PROFILE.backup)
                .with_context(|| format!("patch {position}"))?;
        let normalised = ampghost_params::conform_to_official(&mut image)
            .with_context(|| format!("patch {position} fails the official ranges"))?;
        println!("patch {position}:    {}", image.name());
        let list = |label: &str, names: &[String]| {
            if !names.is_empty() {
                println!("  {label:<11}{}", names.join(", "));
            }
        };
        list("defaulted:", &report.defaulted);
        list("padded:", &report.padded);
        list("skipped:", &report.skipped);
        list("unknown:", &report.unknown);
        for change in &normalised {
            println!(
                "  normalised: {} {} -> {}",
                change.offset, change.from, change.to
            );
        }
        images.push((position, image));
    }
    Ok(images)
}

fn import_presets(
    mut presets: FileStorage,
    file: &Path,
    index: Option<usize>,
    name: Option<&str>,
    replace: bool,
    dry_run: bool,
) -> Result<()> {
    let images = load_tsl_images(file, index)?;
    if name.is_some() && images.len() != 1 {
        bail!("--name needs a single patch, pick one with --index");
    }
    let existing = presets.list()?;
    let mut planned = Vec::new();
    for (position, image) in images {
        let name = name.map_or_else(
            || {
                let patch = image.name();
                if patch.trim().is_empty() {
                    format!("Imported patch {position}")
                } else {
                    patch
                }
            },
            ToOwned::to_owned,
        );
        let key = preset_key(&name)?;
        if planned.iter().any(|(other, _)| *other == key) {
            bail!(
                "two patches map to preset {key}; import them one by one with --index and --name"
            );
        }
        if !replace && existing.contains(&key) {
            bail!("preset {key} already exists; pass --replace to overwrite it");
        }
        let preset = Preset::new(name, PROFILE.name, SystemClock.timestamp(), image)?;
        planned.push((key, preset));
    }
    for (key, preset) in planned {
        if dry_run {
            println!("would save preset {:?} as {key}", preset.name);
            continue;
        }
        let encoded = encode_preset(&preset, PROFILE.temp_patch)?;
        presets.write(&key, encoded.as_bytes())?;
        println!("saved preset {:?} as {key}", preset.name);
    }
    Ok(())
}

fn alb_info(file: &PathBuf) -> Result<()> {
    let text =
        std::fs::read_to_string(file).with_context(|| format!("reading {}", file.display()))?;
    let alb = decode_alb(&text)?;
    let layout = &PROFILE.backup;
    println!("formatRev:  {}", alb.format_rev);
    println!("device:     {}", alb.device);
    println!("model:      {}", alb.model.as_deref().unwrap_or("-"));
    let mut problems = 0;
    for block in layout.system_blocks {
        match alb
            .backup
            .system
            .iter()
            .find(|entry| entry.name == block.name)
        {
            Some(entry) if entry.data.len() == usize::try_from(block.size).unwrap_or_default() => {}
            Some(entry) => {
                problems += 1;
                println!(
                    "system {}: {} bytes, expected {}",
                    block.name,
                    entry.data.len(),
                    block.size
                );
            }
            None => {
                problems += 1;
                println!("system {}: missing", block.name);
            }
        }
    }
    println!("system:     {} blocks", alb.backup.system.len());
    println!("rows:       {}", alb.backup.user_patches.len());
    for (row, entry) in alb.backup.user_patches.iter().enumerate() {
        let Some(blocks) = entry else {
            println!("row {row}:      null");
            continue;
        };
        for block in layout.patch_blocks {
            match blocks.iter().find(|entry| entry.name == block.name) {
                Some(entry) if entry.data.len() == block.len() => {}
                Some(entry) => {
                    problems += 1;
                    println!(
                        "row {row} {}: {} bytes, expected {}",
                        block.name,
                        entry.data.len(),
                        block.size
                    );
                }
                None => {
                    problems += 1;
                    println!("row {row} {}: missing", block.name);
                }
            }
        }
        let (mut image, _) = blocks_to_image(blocks, &PROFILE.patch_layout, layout)?;
        match ampghost_params::conform_to_official(&mut image) {
            Ok(normalised) => println!(
                "row {row}:      {} ({} blocks, {} fixed values off official)",
                image.name(),
                blocks.len(),
                normalised.len()
            ),
            Err(error) => {
                problems += 1;
                println!("row {row}:      {} ({error})", image.name());
            }
        }
    }
    if problems > 0 {
        bail!("{problems} problem(s) found");
    }
    println!("check:      ok");
    Ok(())
}
