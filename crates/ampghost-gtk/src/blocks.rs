use crate::i18n::tr;
use ampghost_params::{Area, Param, ParamTable};
use ampghost_protocol::PatchImage;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Variant {
    Green,
    Red,
    Yellow,
}

impl Variant {
    pub fn from_label(label: &str) -> Option<Self> {
        match label.trim().to_ascii_lowercase().as_str() {
            "green" => Some(Self::Green),
            "red" => Some(Self::Red),
            "yellow" => Some(Self::Yellow),
            _ => None,
        }
    }

    pub fn css_class(self) -> &'static str {
        match self {
            Self::Green => "variant-green",
            Self::Red => "variant-red",
            Self::Yellow => "variant-yellow",
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Self::Green => "Green Variant",
            Self::Red => "Red Variant",
            Self::Yellow => "Yellow Variant",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Category {
    Drive,
    Modulation,
    Amp,
    Ambient,
    Other,
}

impl Category {
    pub fn css_class(self) -> Option<&'static str> {
        match self {
            Self::Drive => Some("cat-drive"),
            Self::Modulation => Some("cat-modulation"),
            Self::Amp => Some("cat-amp"),
            Self::Ambient => Some("cat-ambient"),
            Self::Other => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Subs {
    Shared,
    Typed,
    Slot,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockSpec {
    pub key: &'static str,
    pub title: &'static str,
    pub chain_label: Option<&'static str>,
    pub on_param: Option<&'static str>,
    pub type_param: Option<&'static str>,
    pub select_param: Option<&'static str>,
    pub param_blocks: &'static [&'static str],
    pub subs: Subs,
    pub icon: &'static str,
    pub category: Category,
    pub passive: bool,
    pub mini: bool,
}

const fn block(
    key: &'static str,
    title: &'static str,
    chain_label: Option<&'static str>,
    on_param: Option<&'static str>,
    type_param: Option<&'static str>,
    select_param: Option<&'static str>,
    param_blocks: &'static [&'static str],
) -> BlockSpec {
    BlockSpec {
        key,
        title,
        chain_label,
        on_param,
        type_param,
        select_param,
        param_blocks,
        subs: Subs::Shared,
        icon: "ampghost-fx-symbolic",
        category: Category::Other,
        passive: false,
        mini: false,
    }
}

const fn look(spec: BlockSpec, icon: &'static str, category: Category) -> BlockSpec {
    BlockSpec {
        icon,
        category,
        ..spec
    }
}

const fn passive(spec: BlockSpec) -> BlockSpec {
    BlockSpec {
        passive: true,
        ..spec
    }
}

const fn mini(spec: BlockSpec) -> BlockSpec {
    BlockSpec {
        passive: true,
        mini: true,
        ..spec
    }
}

const fn typed(spec: BlockSpec) -> BlockSpec {
    BlockSpec {
        subs: Subs::Typed,
        ..spec
    }
}

const fn slotted(spec: BlockSpec) -> BlockSpec {
    BlockSpec {
        subs: Subs::Slot,
        ..spec
    }
}

pub static BLOCKS: [BlockSpec; 16] = [
    look(
        block(
            "booster",
            "Booster",
            Some("OD"),
            Some("booster_on_off"),
            Some("booster_type"),
            Some("select_booster"),
            &["booster"],
        ),
        "ampghost-booster-symbolic",
        Category::Drive,
    ),
    look(
        typed(block(
            "mod",
            "Mod",
            Some("FX1"),
            Some("mod_on_off"),
            Some("mod_type"),
            Some("select_mod"),
            &["mod"],
        )),
        "ampghost-mod-symbolic",
        Category::Modulation,
    ),
    look(
        block(
            "amp",
            "Amp",
            Some("CH_A"),
            None,
            Some("amp_type"),
            None,
            &["amp"],
        ),
        "ampghost-amp-symbolic",
        Category::Amp,
    ),
    look(
        block(
            "eq1",
            "EQ",
            Some("EQ1"),
            Some("eq1_on_off"),
            Some("eq1_selection"),
            None,
            &["eq1"],
        ),
        "ampghost-eq-symbolic",
        Category::Amp,
    ),
    look(
        block(
            "eq2",
            "EQ 2",
            Some("EQ2"),
            Some("eq2_on_off"),
            Some("eq2_selection"),
            None,
            &["eq2"],
        ),
        "ampghost-eq-symbolic",
        Category::Amp,
    ),
    look(
        typed(block(
            "fx",
            "FX",
            Some("FX2"),
            Some("fx_on_off"),
            Some("fx_type"),
            Some("select_fx"),
            &["fx"],
        )),
        "ampghost-fx-symbolic",
        Category::Modulation,
    ),
    look(
        block(
            "delay1",
            "Delay",
            Some("DD1"),
            Some("delay1_on_off"),
            Some("delay1_type"),
            Some("select_delay1"),
            &["delay1"],
        ),
        "ampghost-delay-symbolic",
        Category::Ambient,
    ),
    look(
        block(
            "delay2",
            "Delay 2",
            Some("DD2"),
            Some("delay2_on_off"),
            Some("delay2_type"),
            Some("select_reverb"),
            &["delay2"],
        ),
        "ampghost-delay2-symbolic",
        Category::Ambient,
    ),
    look(
        block(
            "reverb",
            "Reverb",
            Some("RV"),
            Some("reverb_on_off"),
            Some("reverb_type"),
            Some("select_reverb"),
            &["reverb"],
        ),
        "ampghost-reverb-symbolic",
        Category::Ambient,
    ),
    look(
        block(
            "send_return",
            "Send/Return",
            Some("LP"),
            Some("send_return_on_off"),
            None,
            None,
            &["send_return"],
        ),
        "ampghost-send-return-symbolic",
        Category::Other,
    ),
    look(
        block(
            "noise_gate",
            "Noise Gate",
            Some("NS_1"),
            Some("noise_gate_on_off"),
            None,
            None,
            &["noise_gate"],
        ),
        "ampghost-gate-symbolic",
        Category::Amp,
    ),
    passive(look(
        block("pedal_fx", "Pedal FX", Some("PDL"), None, None, None, &[]),
        "ampghost-pedal-fx-symbolic",
        Category::Other,
    )),
    mini(look(
        block("cab", "Cabinet", Some("CAB"), None, None, None, &[]),
        "ampghost-cab-symbolic",
        Category::Other,
    )),
    look(
        block(
            "solo",
            "Solo",
            None,
            Some("solo_on_off"),
            None,
            None,
            &["solo"],
        ),
        "ampghost-solo-symbolic",
        Category::Other,
    ),
    look(
        slotted(block(
            "contour",
            "Contour",
            None,
            Some("contour_off_on"),
            Some("contour_contour_select"),
            None,
            &["contour"],
        )),
        "ampghost-contour-symbolic",
        Category::Other,
    ),
    look(
        block("patch", "Patch", None, None, None, None, &["master", "cab"]),
        "ampghost-patch-symbolic",
        Category::Other,
    ),
];

pub fn spec(key: &str) -> Option<&'static BlockSpec> {
    BLOCKS.iter().find(|spec| spec.key == key)
}

pub fn value(table: &ParamTable, image: &PatchImage, id: &str) -> Option<u16> {
    let param = table.get(id).ok()?;
    param_value(param, image)
}

pub fn param_value(param: &Param, image: &PatchImage) -> Option<u16> {
    let offset = param.patch_offset().ok()?;
    let bytes = (0..u32::from(param.size()))
        .map(|distance| image.get(offset.plus(distance)))
        .collect::<Option<Vec<u8>>>()?;
    param.decode(&bytes).ok()
}

const CHAIN_SLOTS: usize = 20;

fn slot_param(table: &ParamTable, slot: usize) -> Option<&Param> {
    table.get(&format!("chain_blocks_chain_slot_{slot}")).ok()
}

fn spec_for_code(table: &ParamTable, code: u8) -> Option<&'static BlockSpec> {
    let label = slot_param(table, 0)?.describe(u16::from(code));
    BLOCKS
        .iter()
        .find(|spec| spec.chain_label == Some(label.as_str()))
}

pub fn reorderable(spec: &BlockSpec) -> bool {
    spec.chain_label.is_some() && !spec.passive
}

pub fn pattern_label(table: &ParamTable, pattern: &[u8]) -> String {
    pattern
        .iter()
        .map(|&code| {
            spec_for_code(table, code).map_or_else(|| code.to_string(), |spec| tr(spec.title))
        })
        .collect::<Vec<_>>()
        .join(" → ")
}

pub fn chain_codes(table: &ParamTable, image: &PatchImage) -> Option<Vec<u8>> {
    (0..CHAIN_SLOTS)
        .map(|slot| {
            let param = slot_param(table, slot)?;
            u8::try_from(param_value(param, image)?).ok()
        })
        .collect()
}

pub fn chain_order(table: &ParamTable, image: &PatchImage) -> Vec<&'static BlockSpec> {
    let mut order: Vec<&'static BlockSpec> = Vec::new();
    for code in chain_codes(table, image).unwrap_or_default() {
        if let Some(spec) = spec_for_code(table, code)
            && !order.contains(&spec)
        {
            order.push(spec);
        }
    }
    for spec in &BLOCKS {
        if spec.chain_label.is_some() && !order.contains(&spec) {
            order.push(spec);
        }
    }
    order
}

pub fn extras() -> impl Iterator<Item = &'static BlockSpec> {
    BLOCKS.iter().filter(|spec| spec.chain_label.is_none())
}

pub fn variant(table: &ParamTable, image: &PatchImage, spec: &BlockSpec) -> Option<Variant> {
    let param = table.get(spec.select_param?).ok()?;
    let selected = param_value(param, image)?;
    Variant::from_label(&param.describe(selected))
}

pub fn is_on(table: &ParamTable, image: &PatchImage, spec: &BlockSpec) -> Option<bool> {
    spec.on_param
        .and_then(|id| value(table, image, id))
        .map(|value| value != 0)
}

pub fn type_value(table: &ParamTable, image: &PatchImage, spec: &BlockSpec) -> Option<u16> {
    spec.type_param.and_then(|id| value(table, image, id))
}

pub fn type_label(table: &ParamTable, image: &PatchImage, spec: &BlockSpec) -> Option<String> {
    let param = table.get(spec.type_param?).ok()?;
    let value = param_value(param, image)?;
    Some(pretty_label(&param.describe(value)))
}

pub fn pretty_label(label: &str) -> String {
    let trimmed = label.trim().trim_end_matches(':').trim_end();
    if let Some(inner) = trimmed
        .strip_prefix("Var [")
        .and_then(|rest| rest.strip_suffix(']'))
    {
        return format!("{} Variation", title_case(inner));
    }
    if let Some(inner) = trimmed
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        return title_case(inner);
    }
    let shouting = trimmed.chars().any(char::is_alphabetic)
        && !trimmed.chars().any(char::is_lowercase)
        && trimmed
            .chars()
            .filter(|character| character.is_alphabetic())
            .count()
            >= 3;
    if shouting {
        title_case(trimmed)
    } else {
        trimmed.to_owned()
    }
}

fn title_case(text: &str) -> String {
    text.split(' ')
        .map(|word| {
            let mut characters = word.chars();
            characters.next().map_or_else(String::new, |first| {
                first.to_uppercase().collect::<String>() + &characters.as_str().to_lowercase()
            })
        })
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn block_params(
    table: &'static ParamTable,
    image: &PatchImage,
    spec: &BlockSpec,
) -> Vec<&'static Param> {
    let filtered = spec.subs != Subs::Shared;
    let sub = match spec.subs {
        Subs::Shared => None,
        Subs::Typed => type_value(table, image, spec)
            .and_then(|value| table.sub_for_type(value))
            .map(str::to_owned),
        Subs::Slot => {
            type_value(table, image, spec).map(|value| (u32::from(value) + 1).to_string())
        }
    };
    table
        .iter()
        .filter(|param| param.area() == Area::Patch && !param.is_hidden())
        .filter(|param| spec.param_blocks.contains(&param.block()))
        .filter(|param| {
            let id = Some(param.id());
            id != spec.on_param && id != spec.type_param && id != spec.select_param
        })
        .filter(|param| !filtered || param.sub().is_none() || param.sub() == sub.as_deref())
        .collect()
}
