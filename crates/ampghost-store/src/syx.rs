use ampghost_protocol::{DeviceProfile, Incoming, PatchImage, split_messages};

use crate::error::StoreError;

pub fn encode_syx(image: &PatchImage, profile: &DeviceProfile) -> Result<Vec<u8>, StoreError> {
    let packets = image.to_packets(profile.temp_patch)?;
    Ok(packets
        .iter()
        .flat_map(|packet| profile.encode_write(packet))
        .collect())
}

pub fn decode_syx(bytes: &[u8], profile: &'static DeviceProfile) -> Result<PatchImage, StoreError> {
    let mut packets = Vec::new();
    for message in split_messages(bytes) {
        if let Incoming::Dt1(packet) = profile.decode(message)? {
            packets.push(packet);
        }
    }
    Ok(PatchImage::from_packets(
        &profile.patch_layout,
        profile.temp_patch,
        &packets,
    )?)
}
