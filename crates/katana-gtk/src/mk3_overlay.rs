use katana_params::ParamTable;
use katana_protocol::{KATANA_50_MK2, Mk3Colour, Mk3Effect, Mk3State, PatchImage};

use crate::i18n::{tr, trf};

pub const CHAIN: [&str; 6] = ["booster", "mod", "amp", "fx", "delay1", "reverb"];
pub const OUTPUT: [&str; 1] = ["patch"];

const MAPPED: [&str; 15] = [
    "amp_volume",
    "amp_bass",
    "amp_middle",
    "amp_treble",
    "booster_on_off",
    "mod_on_off",
    "fx_on_off",
    "delay1_on_off",
    "reverb_on_off",
    "select_booster",
    "select_mod",
    "select_fx",
    "select_delay1",
    "select_reverb",
    "delay1_time",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mk3Overlay {
    texts: Vec<(&'static str, String)>,
    unknown: Vec<&'static str>,
}

impl Mk3Overlay {
    pub fn text(&self, id: &str) -> Option<&str> {
        self.texts
            .iter()
            .find(|(key, _)| *key == id)
            .map(|(_, text)| text.as_str())
    }

    pub fn shows(&self, id: &str) -> bool {
        (MAPPED.contains(&id) && !self.unknown.contains(&id)) || self.text(id).is_some()
    }
}

const fn ids(effect: Mk3Effect) -> (&'static str, &'static str) {
    match effect {
        Mk3Effect::Booster => ("booster_on_off", "select_booster"),
        Mk3Effect::Mod => ("mod_on_off", "select_mod"),
        Mk3Effect::Fx => ("fx_on_off", "select_fx"),
        Mk3Effect::Delay => ("delay1_on_off", "select_delay1"),
        Mk3Effect::Reverb => ("reverb_on_off", "select_reverb"),
    }
}

const fn select_value(colour: Mk3Colour) -> u16 {
    match colour {
        Mk3Colour::Red => 0,
        Mk3Colour::Green => 1,
        Mk3Colour::Orange => 2,
    }
}

struct Projection<'a> {
    table: &'a ParamTable,
    image: PatchImage,
    unknown: Vec<&'static str>,
}

impl Projection<'_> {
    fn put(&mut self, id: &'static str, value: u16) {
        if !self.try_put(id, value) {
            tracing::debug!(%id, value, "MK III value not shown");
            self.unknown.push(id);
        }
    }

    fn try_put(&mut self, id: &str, value: u16) -> bool {
        let Ok(param) = self.table.get(id) else {
            return false;
        };
        let (Ok(offset), Ok(bytes)) = (param.patch_offset(), param.encode(value)) else {
            return false;
        };
        (0_u32..)
            .zip(bytes)
            .all(|(distance, byte)| self.image.set(offset.plus(distance), byte).is_ok())
    }
}

pub fn project(table: &ParamTable, state: &Mk3State) -> Option<(PatchImage, Mk3Overlay)> {
    let layout = &KATANA_50_MK2.patch_layout;
    let mut image = PatchImage::from_data(layout, vec![0; layout.data_len()]).ok()?;
    if image.set_name(&state.name).is_err() {
        tracing::debug!(name = %state.name, "MK III patch name not shown");
    }
    let mut out = Projection {
        table,
        image,
        unknown: Vec::new(),
    };
    out.put("amp_volume", u16::from(state.volume));
    out.put("amp_bass", u16::from(state.bass));
    out.put("amp_middle", u16::from(state.middle));
    out.put("amp_treble", u16::from(state.treble));
    out.put("delay1_time", state.delay_time_ms);
    for effect in &state.effects {
        let (on, select) = ids(effect.effect);
        out.put(on, u16::from(effect.on));
        match effect.colour {
            Some(colour) => out.put(select, select_value(colour)),
            None => out.unknown.push(select),
        }
    }
    let mut texts = vec![("amp_gain", state.gain.to_string())];
    if let Some(amp) = state.amp_type {
        let name = tr(amp.label());
        let text = if state.variation {
            trf("{type} Variation", &[("type", &name)])
        } else {
            name
        };
        texts.push(("amp_type", text));
    }
    Some((
        out.image,
        Mk3Overlay {
            texts,
            unknown: out.unknown,
        },
    ))
}
