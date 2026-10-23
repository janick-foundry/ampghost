use katana_protocol::{Address, Dt1, PatchImage, PatchLayout};
use serde::{Deserialize, Serialize};

use crate::error::StoreError;

pub const PRESET_FORMAT: &str = "katana-control-preset";
pub const PRESET_VERSION: u32 = 1;

const NAME_LIMIT: usize = 64;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    pub name: String,
    pub model: String,
    pub created: String,
    pub image: PatchImage,
}

impl Preset {
    pub fn new(
        name: impl Into<String>,
        model: impl Into<String>,
        created: impl Into<String>,
        image: PatchImage,
    ) -> Result<Self, StoreError> {
        let name = name.into();
        let valid = !name.trim().is_empty()
            && name.chars().count() <= NAME_LIMIT
            && name.chars().all(|character| !character.is_control());
        if !valid {
            return Err(StoreError::InvalidName);
        }
        Ok(Self {
            name,
            model: model.into(),
            created: created.into(),
            image,
        })
    }
}

#[derive(Debug, Serialize, Deserialize)]
struct PresetFile {
    format: String,
    version: u32,
    name: String,
    model: String,
    created: String,
    patch_name: String,
    base: String,
    data_bytes: usize,
    packets: Vec<PacketFile>,
}

#[derive(Debug, Serialize, Deserialize)]
struct PacketFile {
    address: String,
    data: String,
}

pub fn encode_preset(preset: &Preset, base: Address) -> Result<String, StoreError> {
    let packets = preset
        .image
        .to_packets(base)?
        .iter()
        .map(|packet| PacketFile {
            address: hex(&packet.address().bytes()),
            data: hex(packet.data()),
        })
        .collect();
    let file = PresetFile {
        format: PRESET_FORMAT.to_owned(),
        version: PRESET_VERSION,
        name: preset.name.clone(),
        model: preset.model.clone(),
        created: preset.created.clone(),
        patch_name: preset.image.name(),
        base: hex(&base.bytes()),
        data_bytes: preset.image.data().len(),
        packets,
    };
    Ok(serde_json::to_string_pretty(&file)?)
}

pub fn decode_preset(
    text: &str,
    layout: &'static PatchLayout,
    base: Address,
) -> Result<Preset, StoreError> {
    let file: PresetFile = serde_json::from_str(text)?;
    if file.format != PRESET_FORMAT || file.version != PRESET_VERSION {
        return Err(StoreError::Format {
            format: file.format,
            version: file.version,
        });
    }
    let packets = file
        .packets
        .iter()
        .map(|packet| {
            let address = Address::from_bytes(&unhex(&packet.address)?)?;
            Ok(Dt1::new(address, unhex(&packet.data)?)?)
        })
        .collect::<Result<Vec<_>, StoreError>>()?;
    let image = PatchImage::from_packets(layout, base, &packets)?;
    Preset::new(file.name, file.model, file.created, image)
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn unhex(text: &str) -> Result<Vec<u8>, StoreError> {
    text.split_ascii_whitespace()
        .map(|token| u8::from_str_radix(token, 16).map_err(|_| StoreError::Hex(token.to_owned())))
        .collect()
}
