use crate::address::Address;
use crate::error::ProtocolError;
use crate::patch::{PatchImage, PatchOffset, Span};
use crate::sysex::Dt1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Phase {
    Pages,
    LatePage,
    Chain,
}

pub fn plan_full_write(
    image: &PatchImage,
    base: Address,
    late_page: u32,
    chain: Span,
    max_packet: usize,
) -> Result<Vec<Dt1>, ProtocolError> {
    let offsets = image.layout().offsets().collect::<Vec<_>>();
    plan_partial_write(image, base, &offsets, late_page, chain, max_packet)
}

pub fn plan_partial_write(
    image: &PatchImage,
    base: Address,
    offsets: &[PatchOffset],
    late_page: u32,
    chain: Span,
    max_packet: usize,
) -> Result<Vec<Dt1>, ProtocolError> {
    let mut sorted = offsets.to_vec();
    sorted.sort_by_key(|&offset| (phase_of(offset, late_page, chain), offset));
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
                && phase_of(last, late_page, chain) == phase_of(offset, late_page, chain)
        });
        match runs.last_mut() {
            Some((_, bytes)) if extends && bytes.len() < max_packet => bytes.push(value),
            _ => runs.push((offset, vec![value])),
        }
        previous = Some(offset);
    }
    runs.into_iter()
        .map(|(start, bytes)| Dt1::new(start.address(base)?, bytes))
        .collect()
}

fn phase_of(offset: PatchOffset, late_page: u32, chain: Span) -> Phase {
    if chain.contains(offset) {
        Phase::Chain
    } else if offset.page() == late_page {
        Phase::LatePage
    } else {
        Phase::Pages
    }
}
