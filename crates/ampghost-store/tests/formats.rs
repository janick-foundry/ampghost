use std::error::Error;

use ampghost_protocol::{KATANA_50_MK2, PatchImage};
use ampghost_store::{
    FullBackup, LiveSetPatch, NamedBlock, StoreError, TSL_DEVICE, TSL_FORMAT_REV, TSL_MAX_PATCHES,
    decode_alb, decode_syx, decode_tsl, encode_alb, encode_syx, encode_tsl,
};

type TestResult = Result<(), Box<dyn Error>>;

fn sample_image() -> Result<PatchImage, Box<dyn Error>> {
    let layout = &KATANA_50_MK2.patch_layout;
    let data = (0..layout.data_len())
        .map(|index| u8::try_from(index % 128))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PatchImage::from_data(layout, data)?)
}

#[test]
fn syx_round_trips_temp_image() -> TestResult {
    let image = sample_image()?;
    let bytes = encode_syx(&image, KATANA_50_MK2.temp_patch)?;
    assert_eq!(bytes.first(), Some(&0xF0));
    assert_eq!(bytes.last(), Some(&0xF7));
    let decoded = decode_syx(
        &bytes,
        &KATANA_50_MK2.patch_layout,
        KATANA_50_MK2.temp_patch,
    )?;
    assert_eq!(decoded, image);
    Ok(())
}

#[test]
fn syx_rejects_truncated_file() -> TestResult {
    let bytes = encode_syx(&sample_image()?, KATANA_50_MK2.temp_patch)?;
    let half = bytes.get(..bytes.len() / 2).unwrap_or_default();
    assert!(decode_syx(half, &KATANA_50_MK2.patch_layout, KATANA_50_MK2.temp_patch).is_err());
    Ok(())
}

fn patch(memo: &str) -> LiveSetPatch {
    LiveSetPatch {
        memo: memo.to_owned(),
        blocks: vec![NamedBlock {
            name: "PatchName".to_owned(),
            data: vec![0x41; 16],
        }],
    }
}

#[test]
fn tsl_round_trips_and_limits_patch_count() -> TestResult {
    let text = encode_tsl("Set", &[patch("one"), patch("two")])?;
    let set = decode_tsl(&text)?;
    assert_eq!(set.name, "Set");
    assert_eq!(set.format_rev, TSL_FORMAT_REV);
    assert_eq!(set.patches.len(), 2);
    assert!(matches!(encode_tsl("Set", &[]), Err(StoreError::NoPatches)));
    let many = vec![patch("x"); TSL_MAX_PATCHES + 1];
    assert!(matches!(
        encode_tsl("Set", &many),
        Err(StoreError::TooManyPatches(16, 15))
    ));
    Ok(())
}

#[test]
fn tsl_rejects_other_devices_and_revisions() -> TestResult {
    let text = encode_tsl("Set", &[patch("one")])?;
    let gen3 = text.replace(TSL_DEVICE, "KATANA3");
    assert!(matches!(decode_tsl(&gen3), Err(StoreError::Device(device)) if device == "KATANA3"));
    let future = text.replace(TSL_FORMAT_REV, "0009");
    assert!(matches!(decode_tsl(&future), Err(StoreError::FormatRev(_))));
    let bom = format!("\u{feff}{text}");
    assert_eq!(decode_tsl(&bom)?.patches.len(), 1);
    Ok(())
}

#[test]
fn alb_round_trips_and_checks_rows() -> TestResult {
    let backup = FullBackup {
        system: vec![NamedBlock {
            name: "Setup".to_owned(),
            data: vec![0x00, 0x7F],
        }],
        user_patches: vec![None; 9],
    };
    let file = decode_alb(&encode_alb(&backup)?)?;
    assert_eq!(file.backup.user_patches.len(), 9);
    let short = FullBackup {
        system: backup.system.clone(),
        user_patches: vec![None; 8],
    };
    assert!(matches!(
        encode_alb(&short),
        Err(StoreError::RowCount(8, 9))
    ));
    Ok(())
}
