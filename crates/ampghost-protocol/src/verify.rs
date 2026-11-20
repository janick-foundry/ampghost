use std::collections::BTreeSet;
use std::fmt;

use crate::patch::{PatchImage, PatchOffset, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LinkedType {
    pub block: &'static str,
    pub type_at: PatchOffset,
    pub variants_at: PatchOffset,
    pub select_at: PatchOffset,
}

impl LinkedType {
    pub fn active_variant(&self, image: &PatchImage) -> Option<PatchOffset> {
        let selection = image.get(self.select_at)?;
        (selection < 3).then(|| self.variants_at.plus(u32::from(selection)))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerifyRules {
    pub amp_derived: &'static [Span],
    pub linked_types: &'static [LinkedType],
}

impl VerifyRules {
    pub fn is_amp_derived(&self, offset: PatchOffset) -> bool {
        self.amp_derived.iter().any(|span| span.contains(offset))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteMismatch {
    pub offset: PatchOffset,
    pub expected: u8,
    pub actual: u8,
}

impl fmt::Display for ByteMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: wrote {:02X}, read {:02X}",
            self.offset, self.expected, self.actual
        )
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VerifyReport {
    pub mismatches: Vec<ByteMismatch>,
    pub amp_derived_changes: Vec<ByteMismatch>,
    pub linked_accepted: Vec<ByteMismatch>,
}

impl VerifyReport {
    pub fn is_ok(&self) -> bool {
        self.mismatches.is_empty()
    }

    pub fn mismatch_offsets(&self) -> Vec<PatchOffset> {
        self.mismatches
            .iter()
            .map(|mismatch| mismatch.offset)
            .collect()
    }
}

pub fn verify(expected: &PatchImage, actual: &PatchImage, rules: &VerifyRules) -> VerifyReport {
    let accepted = accepted_linked_offsets(expected, actual, rules);
    let mut report = VerifyReport::default();
    for offset in expected.layout().offsets() {
        let (Some(want), Some(got)) = (expected.get(offset), actual.get(offset)) else {
            continue;
        };
        if want == got {
            continue;
        }
        let mismatch = ByteMismatch {
            offset,
            expected: want,
            actual: got,
        };
        if rules.is_amp_derived(offset) {
            report.amp_derived_changes.push(mismatch);
        } else if accepted.contains(&offset) {
            report.linked_accepted.push(mismatch);
        } else {
            report.mismatches.push(mismatch);
        }
    }
    report
}

fn accepted_linked_offsets(
    expected: &PatchImage,
    actual: &PatchImage,
    rules: &VerifyRules,
) -> BTreeSet<PatchOffset> {
    let mut accepted = BTreeSet::new();
    for link in rules.linked_types {
        let Some(variant_at) = link.active_variant(actual) else {
            continue;
        };
        let (Some(actual_type), Some(actual_variant)) =
            (actual.get(link.type_at), actual.get(variant_at))
        else {
            continue;
        };
        let consistent = actual_type == actual_variant;
        let one_matches = expected.get(link.type_at) == Some(actual_type)
            || expected.get(variant_at) == Some(actual_variant);
        if consistent && one_matches {
            accepted.insert(link.type_at);
            accepted.insert(variant_at);
        }
    }
    accepted
}
