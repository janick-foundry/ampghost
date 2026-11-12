use crate::address::{Address, Size};
use crate::error::ProtocolError;
use crate::patch::{PatchOffset, Span};
use crate::sysex::Rq1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PatchBlock {
    pub name: &'static str,
    pub offset: PatchOffset,
    pub size: u32,
}

impl PatchBlock {
    pub const fn span(&self) -> Span {
        Span::new(self.offset, self.size)
    }

    pub fn len(&self) -> usize {
        usize::try_from(self.size).unwrap_or_default()
    }

    pub const fn is_empty(&self) -> bool {
        self.size == 0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SystemBlock {
    pub name: &'static str,
    pub address: Address,
    pub size: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackupLayout {
    pub system_blocks: &'static [SystemBlock],
    pub patch_blocks: &'static [PatchBlock],
    pub status_block: &'static str,
    pub stored_patch_rows: &'static [u8],
    pub stored_patch_slots: u8,
    pub stored_patch_page: u8,
    pub read_chunk: u32,
}

impl BackupLayout {
    pub fn stored_patch_base(&self, row: u8) -> Result<Address, ProtocolError> {
        if row >= self.stored_patch_slots {
            return Err(ProtocolError::UnknownPatchRow(row));
        }
        Ok(Address::from_parts(self.stored_patch_page, row, 0x00, 0x00))
    }

    pub fn patch_block(&self, name: &str) -> Option<&'static PatchBlock> {
        self.patch_blocks.iter().find(|block| block.name == name)
    }

    pub fn chunk_requests(&self, address: Address, size: u32) -> Result<Vec<Rq1>, ProtocolError> {
        let limit = self.read_chunk.max(1);
        let mut requests = Vec::new();
        let mut done = 0;
        while done < size {
            let len = (size - done).min(limit);
            requests.push(Rq1::new(address.offset(done)?, Size::from_linear(len)?));
            done += len;
        }
        Ok(requests)
    }
}

const fn patch(name: &'static str, page: u8, byte: u8, size: u32) -> PatchBlock {
    PatchBlock {
        name,
        offset: PatchOffset::new(page, byte),
        size,
    }
}

const fn system(name: &'static str, b: u8, c: u8, d: u8, size: u32) -> SystemBlock {
    SystemBlock {
        name,
        address: Address::from_parts(0x00, b, c, d),
        size,
    }
}

pub const MK2_SYSTEM_BLOCKS: [SystemBlock; 12] = [
    system("System", 0x00, 0x00, 0x00, 46),
    system("Info", 0x01, 0x00, 0x00, 2),
    system("Midi", 0x02, 0x00, 0x00, 29),
    system("LineoutCustom", 0x00, 0x01, 0x10, 2),
    system("LineoutCustomSetting(1)", 0x00, 0x01, 0x12, 3),
    system("LineoutCustomSetting(2)", 0x00, 0x01, 0x18, 3),
    system("GafcSetting", 0x00, 0x01, 0x20, 5),
    system("SysPwrAdj", 0x00, 0x00, 0x2F, 1),
    system("SysEqSel", 0x00, 0x00, 0x2E, 1),
    system("SysEq(1)", 0x00, 0x00, 0x30, 24),
    system("SysEq(2)", 0x00, 0x00, 0x50, 24),
    system("SysEq(3)", 0x00, 0x00, 0x70, 24),
];

pub const MK2_PATCH_BLOCKS: [PatchBlock; 31] = [
    patch("PatchName", 0x00, 0x00, 16),
    patch("Patch_0", 0x00, 0x10, 72),
    patch("Fx(1)", 0x01, 0x00, 225),
    patch("Fx(2)", 0x03, 0x00, 225),
    patch("Delay(1)", 0x05, 0x00, 26),
    patch("Delay(2)", 0x05, 0x20, 26),
    patch("Patch_1", 0x05, 0x40, 91),
    patch("Patch_2", 0x06, 0x20, 36),
    patch("Status", 0x06, 0x50, 18),
    patch("KnobAsgn", 0x07, 0x00, 34),
    patch("ExpPedalAsgn", 0x08, 0x00, 34),
    patch("ExpPedalAsgnMinMax", 0x08, 0x30, 78),
    patch("GafcExp1Asgn", 0x09, 0x00, 34),
    patch("GafcExp1AsgnMinMax", 0x09, 0x30, 78),
    patch("GafcExp2Asgn", 0x0A, 0x00, 34),
    patch("GafcExp2AsgnMinMax", 0x0A, 0x30, 78),
    patch("FsAsgn", 0x0F, 0x08, 2),
    patch("Patch_Mk2V2", 0x0F, 0x10, 22),
    patch("Contour(1)", 0x0F, 0x30, 2),
    patch("Contour(2)", 0x0F, 0x38, 2),
    patch("Contour(3)", 0x0F, 0x40, 2),
    patch("GafcExp3Asgn", 0x0B, 0x00, 34),
    patch("GafcExp3AsgnMinMax", 0x0B, 0x30, 78),
    patch("GafcExExp1Asgn", 0x0C, 0x00, 34),
    patch("GafcExExp1AsgnMinMax", 0x0C, 0x30, 78),
    patch("GafcExExp2Asgn", 0x0D, 0x00, 34),
    patch("GafcExExp2AsgnMinMax", 0x0D, 0x30, 78),
    patch("GafcExExp3Asgn", 0x0E, 0x00, 34),
    patch("GafcExExp3AsgnMinMax", 0x0E, 0x30, 78),
    patch("CtrlAsgn", 0x0F, 0x00, 8),
    patch("Eq(2)", 0x00, 0x60, 24),
];
