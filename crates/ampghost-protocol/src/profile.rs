use crate::address::{Address, Size};
use crate::blocks::{BackupLayout, MK2_PATCH_BLOCKS, MK2_SYSTEM_BLOCKS};
use crate::channel::Channel;
use crate::error::ProtocolError;
use crate::identity::Identity;
use crate::patch::{PatchImage, PatchLayout, PatchOffset, Span};
use crate::plan::{WriteOrder, plan_full_write, plan_partial_write};
use crate::sysex::{Dt1, Incoming, Rq1, SysexHeader};
use crate::verify::{LinkedType, VerifyRules};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelRow {
    pub channel: Channel,
    pub row: u8,
    pub reload_from_row: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub struct DeviceProfile {
    pub name: &'static str,
    pub sysex: SysexHeader,
    pub identity_signature: [u8; 9],
    pub temp_patch: Address,
    pub patch_layout: PatchLayout,
    pub current_channel: Address,
    pub current_channel_size: Size,
    pub program_change_map: Address,
    pub program_change_map_size: Size,
    pub editor_mode: Address,
    pub channels: &'static [Channel],
    pub channel_rows: &'static [ChannelRow],
    pub verify_rules: VerifyRules,
    pub write_order: WriteOrder,
    pub chain_pattern: PatchOffset,
    pub chain_patterns: &'static [[u8; 5]],
    pub backup: BackupLayout,
}

impl DeviceProfile {
    pub fn matches(&self, identity: &Identity) -> bool {
        identity.signature() == self.identity_signature
    }

    pub fn encode_request(&self, request: &Rq1) -> Vec<u8> {
        request.encode(&self.sysex)
    }

    pub fn encode_write(&self, packet: &Dt1) -> Vec<u8> {
        packet.encode(&self.sysex)
    }

    pub fn decode(&self, message: &[u8]) -> Result<Incoming, ProtocolError> {
        self.sysex.decode(message)
    }

    pub fn read_temp_request(&self) -> Rq1 {
        Rq1::new(self.temp_patch, self.patch_layout.size())
    }

    pub fn read_current_channel_request(&self) -> Rq1 {
        Rq1::new(self.current_channel, self.current_channel_size)
    }

    pub fn read_program_change_map_request(&self) -> Rq1 {
        Rq1::new(self.program_change_map, self.program_change_map_size)
    }

    pub fn editor_mode_on(&self) -> Result<Dt1, ProtocolError> {
        Dt1::new(self.editor_mode, [EDITOR_MODE_ON])
    }

    pub fn editor_mode_off(&self) -> Result<Dt1, ProtocolError> {
        Dt1::new(self.editor_mode, [EDITOR_MODE_OFF])
    }

    pub fn has_channel(&self, channel: Channel) -> bool {
        self.channels.contains(&channel)
    }

    pub fn reload_row(&self, channel: Channel) -> Option<u8> {
        self.channel_rows
            .iter()
            .find(|entry| entry.channel == channel && entry.reload_from_row)
            .map(|entry| entry.row)
    }

    pub fn full_write_plan(&self, image: &PatchImage) -> Result<Vec<Dt1>, ProtocolError> {
        let packets = plan_full_write(image, self.temp_patch, &self.write_order)?;
        self.checked(packets)
    }

    pub fn partial_write_plan(
        &self,
        image: &PatchImage,
        offsets: &[PatchOffset],
    ) -> Result<Vec<Dt1>, ProtocolError> {
        let span = self.write_order.effect_chain;
        let offsets = offsets
            .iter()
            .copied()
            .filter(|offset| !span.contains(*offset))
            .collect::<Vec<_>>();
        let packets = plan_partial_write(image, self.temp_patch, &offsets, &self.write_order)?;
        self.checked(packets)
    }

    pub fn chain_order(&self, image: &PatchImage) -> Option<Vec<u8>> {
        self.write_order
            .effect_chain
            .offsets()
            .map(|offset| image.get(offset))
            .collect()
    }

    pub fn chain_pattern_of(&self, image: &PatchImage) -> Option<u8> {
        image
            .get(self.chain_pattern)
            .filter(|&pattern| usize::from(pattern) < self.chain_patterns.len())
    }

    pub fn with_chain_pattern(
        &self,
        image: &PatchImage,
        pattern: u8,
    ) -> Result<PatchImage, ProtocolError> {
        if usize::from(pattern) >= self.chain_patterns.len() {
            return Err(ProtocolError::InvalidChainPattern {
                pattern,
                count: self.chain_patterns.len(),
            });
        }
        let mut target = image.clone();
        target.set(self.chain_pattern, pattern)?;
        Ok(target)
    }

    pub fn chain_write_plan(&self, image: &PatchImage) -> Result<Vec<Dt1>, ProtocolError> {
        let pattern = self
            .chain_pattern_of(image)
            .ok_or(ProtocolError::OutsideLayout(self.chain_pattern.linear()))?;
        let packet = Dt1::new(self.chain_pattern.address(self.temp_patch)?, [pattern])?;
        self.checked(vec![packet])
    }

    pub fn check_write(&self, packet: &Dt1) -> Result<(), ProtocolError> {
        let forbidden = || ProtocolError::WriteForbidden {
            address: packet.address(),
            len: packet.data().len(),
        };
        if packet.address() == self.editor_mode {
            return match packet.data() {
                [EDITOR_MODE_OFF | EDITOR_MODE_ON] => Ok(()),
                _ => Err(forbidden()),
            };
        }
        let start = packet
            .address()
            .distance_from(self.temp_patch)
            .ok_or_else(forbidden)?;
        let inside = (start
            ..start + u32::try_from(packet.data().len()).map_err(|_| forbidden())?)
            .all(|position| {
                self.patch_layout
                    .contains(PatchOffset::from_linear(position))
            });
        if inside && packet.address().bytes().first() == self.temp_patch.bytes().first() {
            Ok(())
        } else {
            Err(forbidden())
        }
    }

    fn checked(&self, packets: Vec<Dt1>) -> Result<Vec<Dt1>, ProtocolError> {
        for packet in &packets {
            self.check_write(packet)?;
        }
        Ok(packets)
    }
}

const EDITOR_MODE_OFF: u8 = 0x00;
const EDITOR_MODE_ON: u8 = 0x01;

const MK2_RETURNED: [Span; 2] = [
    Span::between(PatchOffset::new(0x00, 0x00), PatchOffset::new(0x07, 0x43)),
    Span::between(PatchOffset::new(0x08, 0x00), PatchOffset::new(0x0F, 0x47)),
];

const MK2_AMP_DERIVED: [Span; 2] = [
    Span::between(PatchOffset::new(0x06, 0x00), PatchOffset::new(0x06, 0x13)),
    Span::between(PatchOffset::new(0x06, 0x50), PatchOffset::new(0x06, 0x61)),
];

const MK2_CHAIN_PATTERNS: [[u8; 5]; 7] = [
    [15, 2, 5, 6, 7],
    [15, 5, 2, 6, 7],
    [15, 5, 6, 2, 7],
    [15, 5, 6, 7, 2],
    [5, 15, 2, 6, 7],
    [5, 15, 6, 2, 7],
    [5, 15, 6, 7, 2],
];

const fn linked(block: &'static str, type_at: PatchOffset, index: u8, select_at: u8) -> LinkedType {
    LinkedType {
        block,
        type_at,
        variants_at: PatchOffset::new(0x06, 0x24 + 3 * index),
        select_at: PatchOffset::new(0x06, select_at),
    }
}

const MK2_LINKED: [LinkedType; 6] = [
    linked("booster", PatchOffset::new(0x00, 0x11), 0, 0x39),
    linked("mod", PatchOffset::new(0x01, 0x01), 1, 0x3A),
    linked("fx", PatchOffset::new(0x03, 0x01), 2, 0x3B),
    linked("delay1", PatchOffset::new(0x05, 0x01), 3, 0x3C),
    linked("reverb", PatchOffset::new(0x05, 0x41), 4, 0x3D),
    linked("delay2", PatchOffset::new(0x05, 0x21), 5, 0x3D),
];

const MK2_50_CHANNEL_ROWS: [ChannelRow; 5] = [
    ChannelRow {
        channel: Channel::Panel,
        row: 0,
        reload_from_row: false,
    },
    ChannelRow {
        channel: Channel::A1,
        row: 1,
        reload_from_row: true,
    },
    ChannelRow {
        channel: Channel::A2,
        row: 2,
        reload_from_row: true,
    },
    ChannelRow {
        channel: Channel::B1,
        row: 5,
        reload_from_row: true,
    },
    ChannelRow {
        channel: Channel::B2,
        row: 6,
        reload_from_row: true,
    },
];

pub static KATANA_50_MK2: DeviceProfile = DeviceProfile {
    name: "BOSS Katana 50 MK II",
    sysex: SysexHeader::new(0x00, &[0x00, 0x00, 0x00, 0x33]),
    identity_signature: [0x41, 0x33, 0x03, 0x00, 0x00, 0x05, 0x00, 0x00, 0x00],
    temp_patch: Address::from_parts(0x60, 0x00, 0x00, 0x00),
    patch_layout: PatchLayout::new(Size::from_parts(0x00, 0x00, 0x0F, 0x48), &MK2_RETURNED, 241),
    current_channel: Address::from_parts(0x00, 0x01, 0x00, 0x00),
    current_channel_size: Size::from_parts(0x00, 0x00, 0x00, 0x02),
    program_change_map: Address::from_parts(0x00, 0x02, 0x00, 0x0C),
    program_change_map_size: Size::from_parts(0x00, 0x00, 0x00, 0x09),
    editor_mode: Address::from_parts(0x7F, 0x00, 0x00, 0x01),
    channels: &[
        Channel::Panel,
        Channel::A1,
        Channel::A2,
        Channel::B1,
        Channel::B2,
    ],
    channel_rows: &MK2_50_CHANNEL_ROWS,
    verify_rules: VerifyRules {
        amp_derived: &MK2_AMP_DERIVED,
        linked_types: &MK2_LINKED,
    },
    write_order: WriteOrder {
        type_selects: Span::between(PatchOffset::new(0x06, 0x24), PatchOffset::new(0x06, 0x3D)),
        late_page: 0x06,
        effect_chain: Span::between(PatchOffset::new(0x06, 0x00), PatchOffset::new(0x06, 0x13)),
        max_packet: 128,
    },
    chain_pattern: PatchOffset::new(0x06, 0x20),
    chain_patterns: &MK2_CHAIN_PATTERNS,
    backup: BackupLayout {
        system_blocks: &MK2_SYSTEM_BLOCKS,
        patch_blocks: &MK2_PATCH_BLOCKS,
        status_block: "Status",
        stored_patch_rows: &[0, 1, 2, 5, 6],
        stored_patch_slots: 9,
        stored_patch_page: 0x10,
        read_chunk: 128,
    },
};

pub static PROFILES: [&DeviceProfile; 1] = [&KATANA_50_MK2];

pub fn profile_for(identity: &Identity) -> Option<&'static DeviceProfile> {
    PROFILES
        .iter()
        .copied()
        .find(|profile| profile.matches(identity))
}
