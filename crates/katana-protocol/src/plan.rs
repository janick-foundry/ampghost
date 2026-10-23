use crate::address::Address;
use crate::error::ProtocolError;
use crate::patch::{PatchImage, PatchOffset, Span};
use crate::sysex::Dt1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteOrder {
    pub type_selects: Span,
    pub late_page: u32,
    pub effect_chain: Span,
    pub max_packet: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    TypeSelects,
    Pages,
    LatePage,
    Chain,
}

impl WriteOrder {
    fn phase_of(&self, offset: PatchOffset) -> Phase {
        if self.type_selects.contains(offset) {
            Phase::TypeSelects
        } else if self.effect_chain.contains(offset) {
            Phase::Chain
        } else if offset.page() == self.late_page {
            Phase::LatePage
        } else {
            Phase::Pages
        }
    }
}

pub fn plan_full_write(
    image: &PatchImage,
    base: Address,
    order: &WriteOrder,
) -> Result<Vec<Dt1>, ProtocolError> {
    let offsets = image.layout().offsets().collect::<Vec<_>>();
    plan_partial_write(image, base, &offsets, order)
}

pub fn plan_partial_write(
    image: &PatchImage,
    base: Address,
    offsets: &[PatchOffset],
    order: &WriteOrder,
) -> Result<Vec<Dt1>, ProtocolError> {
    let mut sorted = offsets.to_vec();
    sorted.sort_by_key(|&offset| (order.phase_of(offset), offset));
    sorted.dedup();
    let mut runs: Vec<(PatchOffset, Vec<u8>)> = Vec::new();
    let mut previous: Option<PatchOffset> = None;
    for offset in sorted {
        let value = image
            .get(offset)
            .ok_or(ProtocolError::OutsideLayout(offset.linear()))?;
        let extends = previous.is_some_and(|last| {
            last.linear() + 1 == offset.linear()
                && last.page() == offset.page()
                && order.phase_of(last) == order.phase_of(offset)
        });
        match runs.last_mut() {
            Some((_, bytes)) if extends && bytes.len() < order.max_packet => bytes.push(value),
            _ => runs.push((offset, vec![value])),
        }
        previous = Some(offset);
    }
    runs.into_iter()
        .map(|(start, bytes)| Dt1::new(start.address(base)?, bytes))
        .collect()
}
