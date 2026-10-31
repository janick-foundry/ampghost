use adw::prelude::*;
use katana_protocol::{Mk3Colour, Mk3Effect, Mk3EffectState, Mk3State};

use crate::i18n::{tr, trf};

const CARD_WIDTH: i32 = 168;
const SPACING: i32 = 12;

struct Level {
    scale: gtk::Scale,
    value: gtk::Label,
}

impl Level {
    fn new() -> Self {
        let scale = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, 100.0, 1.0);
        scale.set_draw_value(false);
        scale.set_hexpand(true);
        scale.set_sensitive(false);
        let value = gtk::Label::builder()
            .width_chars(3)
            .xalign(1.0)
            .css_classes(["numeric", "dim-label"])
            .build();
        Self { scale, value }
    }

    fn row(&self, title: &str) -> gtk::Box {
        let row = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .build();
        let line = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        line.append(
            &gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .hexpand(true)
                .css_classes(["caption"])
                .build(),
        );
        line.append(&self.value);
        row.append(&line);
        row.append(&self.scale);
        row
    }

    fn set(&self, value: u8) {
        self.scale.set_value(f64::from(value));
        self.value.set_text(&value.to_string());
    }
}

struct EffectCard {
    effect: Mk3Effect,
    root: gtk::Box,
    power: gtk::Switch,
    dot: gtk::Box,
    colour: gtk::Label,
    knob: Level,
    time: Option<gtk::Label>,
}

pub struct Mk3View {
    pub root: gtk::ScrolledWindow,
    channel: gtk::Label,
    amp_type: gtk::Label,
    amp: [Level; 5],
    effects: Vec<EffectCard>,
}

fn card_box() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .width_request(CARD_WIDTH)
        .valign(gtk::Align::Start)
        .css_classes(["card"])
        .build()
}

fn card_inner() -> gtk::Box {
    gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .build()
}

fn heading(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .css_classes(["heading"])
        .build()
}

fn effect_name(effect: Mk3Effect) -> String {
    tr(effect.label())
}

fn colour_name(colour: Option<Mk3Colour>) -> String {
    colour.map_or_else(|| tr("Unknown"), |colour| tr(colour.label()))
}

fn colour_class(colour: Option<Mk3Colour>) -> &'static str {
    match colour {
        Some(Mk3Colour::Green) => "variant-green",
        Some(Mk3Colour::Red) => "variant-red",
        Some(Mk3Colour::Orange) => "variant-yellow",
        None => "variant-none",
    }
}

const COLOUR_CLASSES: [&str; 4] = [
    "variant-green",
    "variant-red",
    "variant-yellow",
    "variant-none",
];

impl EffectCard {
    fn new(effect: Mk3Effect) -> Self {
        let root = card_box();
        root.add_css_class("chain-node");
        let inner = card_inner();
        let top = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        let title = heading(&effect_name(effect));
        title.set_hexpand(true);
        title.add_css_class("node-title");
        let power = gtk::Switch::builder()
            .valign(gtk::Align::Center)
            .sensitive(false)
            .build();
        top.append(&title);
        top.append(&power);
        let colour_row = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .css_classes(["node-variant"])
            .build();
        let dot = gtk::Box::builder()
            .valign(gtk::Align::Center)
            .css_classes(["variant-dot", "variant-none"])
            .build();
        let colour = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["caption"])
            .build();
        colour_row.append(&dot);
        colour_row.append(&colour);
        let knob = Level::new();
        inner.append(&top);
        inner.append(&colour_row);
        inner.append(&knob.row(&tr("Panel knob")));
        let time = (effect == Mk3Effect::Delay).then(|| {
            let label = gtk::Label::builder()
                .xalign(0.0)
                .css_classes(["caption", "numeric"])
                .build();
            inner.append(&label);
            label
        });
        root.append(&inner);
        Self {
            effect,
            root,
            power,
            dot,
            colour,
            knob,
            time,
        }
    }

    fn update(&self, state: Mk3EffectState, delay_time_ms: u16) {
        self.power.set_active(state.on);
        if state.on {
            self.root.remove_css_class("block-off");
        } else {
            self.root.add_css_class("block-off");
        }
        for class in COLOUR_CLASSES {
            self.dot.remove_css_class(class);
        }
        self.dot.add_css_class(colour_class(state.colour));
        self.colour.set_text(&colour_name(state.colour));
        self.knob.set(state.knob);
        if let Some(time) = &self.time {
            time.set_text(&trf("Time: {ms} ms", &[("ms", &delay_time_ms.to_string())]));
        }
    }
}

impl Mk3View {
    pub fn new() -> Self {
        let page = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(SPACING)
            .margin_start(24)
            .margin_end(24)
            .margin_top(24)
            .margin_bottom(24)
            .build();
        let notice = gtk::Label::builder()
            .label(tr(
                "This model is shown read only. Values follow the amp panel and cannot be changed here.",
            ))
            .wrap(true)
            .xalign(0.0)
            .css_classes(["dim-label"])
            .build();
        let channel = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["title-4"])
            .build();
        page.append(&channel);
        page.append(&notice);
        page.append(&heading(&tr("Signal Chain")));
        let chain = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(SPACING)
            .build();
        let effects = Mk3Effect::ALL
            .into_iter()
            .map(EffectCard::new)
            .collect::<Vec<_>>();
        let amp_root = card_box();
        amp_root.add_css_class("chain-node");
        let amp_inner = card_inner();
        let amp_title = heading(&tr("Amp"));
        amp_title.add_css_class("node-title");
        let amp_type = gtk::Label::builder()
            .xalign(0.0)
            .css_classes(["caption"])
            .build();
        amp_inner.append(&amp_title);
        amp_inner.append(&amp_type);
        let amp = [
            Level::new(),
            Level::new(),
            Level::new(),
            Level::new(),
            Level::new(),
        ];
        for (level, title) in amp
            .iter()
            .zip(["Gain", "Volume", "Bass", "Middle", "Treble"])
        {
            amp_inner.append(&level.row(&tr(title)));
        }
        amp_root.append(&amp_inner);
        for card in &effects {
            if card.effect == Mk3Effect::Fx {
                chain.append(&amp_root);
            }
            chain.append(&card.root);
        }
        let scroller = gtk::ScrolledWindow::builder()
            .vscrollbar_policy(gtk::PolicyType::Never)
            .child(&chain)
            .build();
        page.append(&scroller);
        let clamp = adw::Clamp::builder()
            .maximum_size(1400)
            .child(&page)
            .build();
        let root = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .child(&clamp)
            .build();
        Self {
            root,
            channel,
            amp_type,
            amp,
            effects,
        }
    }

    pub fn update(&self, state: &Mk3State) {
        let channel = state
            .channel
            .map_or_else(|| tr("Unknown"), |channel| tr(channel.label()));
        self.channel
            .set_text(&trf("Channel: {channel}", &[("channel", &channel)]));
        let amp = state
            .amp_type
            .map_or_else(|| tr("Unknown"), |amp| tr(amp.label()));
        let amp = if state.variation {
            trf("{amp} (Variation)", &[("amp", &amp)])
        } else {
            amp
        };
        self.amp_type.set_text(&amp);
        for (level, value) in self.amp.iter().zip([
            state.gain,
            state.volume,
            state.bass,
            state.middle,
            state.treble,
        ]) {
            level.set(value);
        }
        for card in &self.effects {
            if let Some(effect) = state.effect(card.effect) {
                card.update(*effect, state.delay_time_ms);
            }
        }
    }
}
