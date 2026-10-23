use katana_protocol::{Address, Incoming, PatchImage, PatchLayout, decode, split_messages};

use crate::error::StoreError;

pub fn encode_syx(image: &PatchImage, base: Address) -> Result<Vec<u8>, StoreError> {
    let packets = image.to_packets(base)?;
    Ok(packets
        .iter()
        .flat_map(katana_protocol::Dt1::encode)
        .collect())
}

pub fn decode_syx(
    bytes: &[u8],
    layout: &'static PatchLayout,
    base: Address,
) -> Result<PatchImage, StoreError> {
    let mut packets = Vec::new();
    for message in split_messages(bytes) {
        if let Incoming::Dt1(packet) = decode(message)? {
            packets.push(packet);
        }
    }
    Ok(PatchImage::from_packets(layout, base, &packets)?)
}
