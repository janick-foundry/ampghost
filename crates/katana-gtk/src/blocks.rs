use katana_params::{Area, Param, ParamTable};
use katana_protocol::PatchImage;

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
pub struct BlockSpec {
    pub key: &'static str,
    pub title: &'static str,
    pub chain_label: Option<&'static str>,
    pub on_param: Option<&'static str>,
    pub type_param: Option<&'static str>,
    pub select_param: Option<&'static str>,
    pub param_blocks: &'static [&'static str],
    pub typed_subs: bool,
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
        typed_subs: false,
    }
}

const fn typed(spec: BlockSpec) -> BlockSpec {
    BlockSpec {
        typed_subs: true,
        ..spec
    }
}

pub static BLOCKS: [BlockSpec; 14] = [
    block(
        "booster",
        "Booster",
        Some("OD"),
        Some("booster_on_off"),
        Some("booster_type"),
        Some("select_booster"),
        &["booster"],
    ),
    typed(block(
        "mod",
        "Mod",
        Some("FX1"),
        Some("mod_on_off"),
        Some("mod_type"),
        Some("select_mod"),
        &["mod"],
    )),
    block(
        "amp",
        "Amp",
        Some("CH_A"),
        None,
        Some("amp_type"),
        None,
        &["amp"],
    ),
    block(
        "eq1",
        "EQ",
        Some("EQ1"),
        Some("eq1_on_off"),
        Some("eq1_selection"),
        None,
        &["eq1"],
    ),
    block(
        "eq2",
        "EQ 2",
        Some("EQ2"),
        Some("eq2_on_off"),
        Some("eq2_selection"),
        None,
        &["eq2"],
    ),
    typed(block(
        "fx",
        "FX",
        Some("FX2"),
        Some("fx_on_off"),
        Some("fx_type"),
        Some("select_fx"),
        &["fx"],
    )),
    block(
        "delay1",
        "Delay",
        Some("DD1"),
        Some("delay1_on_off"),
        Some("delay1_type"),
        Some("select_delay1"),
        &["delay1"],
    ),
    block(
        "delay2",
        "Delay 2",
        Some("DD2"),
        Some("delay2_on_off"),
        Some("delay2_type"),
        Some("select_reverb"),
        &["delay2"],
    ),
    block(
        "reverb",
        "Reverb",
        Some("RV"),
        Some("reverb_on_off"),
        Some("reverb_type"),
        Some("select_reverb"),
        &["reverb"],
    ),
    block(
        "send_return",
        "Send/Return",
        Some("LP"),
        Some("send_return_on_off"),
        None,
        None,
        &["send_return"],
    ),
    block(
        "noise_gate",
        "Noise Gate",
        Some("NS_1"),
        Some("noise_gate_on_off"),
        None,
        None,
        &["noise_gate"],
    ),
    block(
        "solo",
        "Solo",
        None,
        Some("solo_on_off"),
        None,
        None,
        &["solo"],
    ),
    block(
        "contour",
        "Contour",
        None,
        Some("contour_off_on"),
        None,
        None,
        &["contour"],
    ),
    block("patch", "Patch", None, None, None, None, &["master", "cab"]),
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

pub fn chain_order(table: &ParamTable, image: &PatchImage) -> Vec<&'static BlockSpec> {
    let mut order: Vec<&'static BlockSpec> = Vec::new();
    for slot in 0..20 {
        let id = format!("chain_blocks_chain_slot_{slot}");
        let Ok(param) = table.get(&id) else {
            continue;
        };
        let Some(code) = param_value(param, image) else {
            continue;
        };
        let label = param.describe(code);
        if let Some(spec) = BLOCKS
            .iter()
            .find(|spec| spec.chain_label == Some(label.as_str()))
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
    let sub = if spec.typed_subs {
        type_value(table, image, spec).and_then(|value| table.sub_for_type(value))
    } else {
        None
    };
    table
        .iter()
        .filter(|param| param.area() == Area::Patch)
        .filter(|param| spec.param_blocks.contains(&param.block()))
        .filter(|param| {
            let id = Some(param.id());
            id != spec.on_param && id != spec.type_param && id != spec.select_param
        })
        .filter(|param| !spec.typed_subs || param.sub().is_none() || param.sub() == sub)
        .collect()
}
