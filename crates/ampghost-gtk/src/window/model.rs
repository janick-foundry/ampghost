use ampghost_engine::{DeviceInfo, PresetEntry};
use ampghost_protocol::{Channel, DeviceProfile, PatchImage};

#[derive(Default)]
pub(super) struct Model {
    pub(super) device: Option<DeviceInfo>,
    pub(super) channel: Option<Channel>,
    pub(super) image: Option<PatchImage>,
    pub(super) baseline: Option<PatchImage>,
    pub(super) presets: Vec<PresetEntry>,
    pub(super) chosen_preset: Option<String>,
    pub(super) selected: &'static str,
    pub(super) pending_switch: Option<Channel>,
    pub(super) failed_detect: bool,
    pub(super) held: Option<Held>,
    pub(super) lost_edits: Option<PatchImage>,
}

pub(super) struct Held {
    pub(super) image: PatchImage,
    pub(super) baseline: Option<PatchImage>,
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub(super) fn editable_differs(
    profile: Option<&'static DeviceProfile>,
    image: &PatchImage,
    other: &PatchImage,
) -> bool {
    image
        .differing_offsets(other)
        .iter()
        .any(|offset| profile.is_none_or(|profile| !profile.verify_rules.is_amp_derived(*offset)))
}

pub(super) fn writable(model: &Model) -> bool {
    model.image.is_some() && model.device.as_ref().is_some_and(DeviceInfo::is_supported)
}
