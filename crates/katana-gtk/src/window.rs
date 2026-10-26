use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use adw::prelude::*;
use anyhow::{Context, Result};
use gtk::{gio, glib};
use katana_engine::{
    AmpSwitch, ChainOutcome, Command, DetectReport, DeviceInfo, EngineError, EngineHandle, Event,
    ParamWrite, PresetEntry, Stage,
};
use katana_params::ParamTable;
use katana_protocol::{Channel, Dt1, PatchImage, PatchOffset, parse_current_channel};

use crate::blocks::{self, BlockSpec, Variant};
use crate::bridge;
use crate::dialogs::{self, SwitchChoice};
use crate::i18n::{tr, trf};
use crate::params_view::{self, Edit, OnChange, ParamsView, Structure};

const EVENT_PUMP: Duration = Duration::from_millis(30);
const CONTINUOUS_WRITE_DEBOUNCE: Duration = Duration::from_millis(500);
const STATUS_DELAY: Duration = Duration::from_millis(300);
const TOGGLE_FADE: u32 = 250;

type Handler = Box<dyn Fn(&Rc<Ui>)>;
type MoveTarget = Rc<dyn Fn(&Ui) -> Option<&'static str>>;

#[derive(Debug, Clone, PartialEq, Eq)]
enum FollowUp {
    ApplyPreset { key: String, name: String },
    Restore { key: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Job {
    Detect,
    ReadTemp,
    WriteParams,
    WriteChain { undo: bool },
    Switch(Channel),
    Backup { then: Option<FollowUp> },
    Restore,
    SavePreset,
    ApplyPreset(String),
    ListPresets,
    ListBackups,
}

impl Job {
    fn shows_status(&self) -> bool {
        !matches!(self, Self::ListPresets | Self::ListBackups)
    }

    fn status_text(&self) -> String {
        match self {
            Self::Detect => tr("Looking for Katana…"),
            Self::ReadTemp => tr("Reading from Katana…"),
            Self::WriteParams => tr("Writing…"),
            Self::WriteChain { undo: false } => tr("Moving block…"),
            Self::WriteChain { undo: true } => tr("Undoing chain change…"),
            Self::Switch(channel) => {
                trf("Switching to {channel}…", &[("channel", channel.label())])
            }
            Self::Backup { then: None } => tr("Backing up temp…"),
            Self::Backup { then: Some(_) } => tr("Backing up current sound…"),
            Self::Restore => tr("Restoring backup…"),
            Self::SavePreset => tr("Saving preset…"),
            Self::ApplyPreset(name) => trf("Applying “{name}”…", &[("name", name)]),
            Self::ListPresets | Self::ListBackups => String::new(),
        }
    }
}

#[derive(Default)]
struct Model {
    device: Option<DeviceInfo>,
    channel: Option<Channel>,
    image: Option<PatchImage>,
    baseline: Option<PatchImage>,
    presets: Vec<PresetEntry>,
    selected: &'static str,
    pending_switch: Option<Channel>,
    failed_detect: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Move {
    Left,
    Right,
    Start,
    End,
}

impl Move {
    const ALL: [Self; 4] = [Self::Left, Self::Right, Self::Start, Self::End];

    fn action(self) -> &'static str {
        match self {
            Self::Left => "move-left",
            Self::Right => "move-right",
            Self::Start => "move-start",
            Self::End => "move-end",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Left => "Move Left",
            Self::Right => "Move Right",
            Self::Start => "Move to Start",
            Self::End => "Move to End",
        }
    }

    fn accel(self) -> &'static str {
        match self {
            Self::Left => "<Alt>Left",
            Self::Right => "<Alt>Right",
            Self::Start => "<Alt>Home",
            Self::End => "<Alt>End",
        }
    }

    fn from_key(key: gtk::gdk::Key) -> Option<Self> {
        match key {
            gtk::gdk::Key::Left | gtk::gdk::Key::KP_Left => Some(Self::Left),
            gtk::gdk::Key::Right | gtk::gdk::Key::KP_Right => Some(Self::Right),
            gtk::gdk::Key::Home | gtk::gdk::Key::KP_Home => Some(Self::Start),
            gtk::gdk::Key::End | gtk::gdk::Key::KP_End => Some(Self::End),
            _ => None,
        }
    }

    fn target(self, from: usize, count: usize) -> Option<usize> {
        match self {
            Self::Left => from.checked_sub(1),
            Self::Right => (from + 1 < count).then_some(from + 1),
            Self::Start => (from > 0).then_some(0),
            Self::End => (from + 1 < count).then(|| count - 1),
        }
    }
}

struct Gap {
    root: gtk::Box,
    indicator: gtk::Box,
}

impl Gap {
    fn new() -> Self {
        let indicator = gtk::Box::builder().css_classes(["drop-indicator"]).build();
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .visible(false)
            .build();
        root.append(&indicator);
        Self { root, indicator }
    }

    fn set_active(&self, active: bool) {
        self.indicator.set_visible(active);
        self.root.set_visible(active);
    }
}

struct Card {
    key: &'static str,
    root: gtk::Overlay,
    actions: Option<gio::SimpleActionGroup>,
    dot: gtk::Box,
    type_label: gtk::Label,
    power: Option<gtk::ToggleButton>,
}

pub struct Ui {
    table: &'static ParamTable,
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    title: adw::WindowTitle,
    status_active: Rc<Cell<bool>>,
    status_label: gtk::Label,
    light: gtk::Box,
    channel_buttons: Vec<(Channel, gtk::Button)>,
    stack: gtk::Stack,
    status_page: adw::StatusPage,
    retry_button: gtk::Button,
    chain_box: gtk::Box,
    extras_box: gtk::Box,
    params_holder: adw::Bin,
    detail_icon: gtk::Image,
    detail_title: gtk::Label,
    detail_subtitle: gtk::Label,
    detail_moves: gtk::Box,
    detail_clamp: adw::Clamp,
    detail_actions: gio::SimpleActionGroup,
    preset_list: gtk::ListBox,
    preset_search: gtk::SearchEntry,
    sidebar: gtk::Box,
    engine: RefCell<Option<EngineHandle>>,
    model: RefCell<Model>,
    jobs: RefCell<VecDeque<Job>>,
    updating: Rc<Cell<bool>>,
    pending: RefCell<BTreeMap<&'static str, u16>>,
    write_timers: RefCell<BTreeMap<&'static str, glib::SourceId>>,
    in_flight: RefCell<BTreeMap<&'static str, u16>>,
    status_serial: Rc<Cell<u64>>,
    closing: Cell<bool>,
    cards: RefCell<Vec<Card>>,
    chain_keys: RefCell<Vec<&'static str>>,
    refocus: Cell<Option<&'static str>>,
    gaps: RefCell<Vec<Gap>>,
    dragging: Cell<Option<&'static str>>,
    drop_gap: Cell<Option<usize>>,
    chain_toast: RefCell<Option<adw::Toast>>,
    params: RefCell<Option<ParamsView>>,
    search_text: Rc<RefCell<String>>,
}

pub fn present(app: &adw::Application) -> Result<()> {
    let table = katana_params::table().context("loading the parameter table")?;
    let bridge = bridge::start()?;
    tracing::info!(data = %bridge.data_dir.display(), "engine started");
    let ui = Ui::build(app, table, bridge.handle);
    install_actions(app, &ui);
    pump_events(&ui, bridge.events);
    let shutdown_ui = Rc::clone(&ui);
    app.connect_shutdown(move |_| shutdown_ui.stop_engine());
    ui.window.present();
    ui.send(Command::ListPresets, Job::ListPresets);
    ui.send(Command::Detect, Job::Detect);
    ui.refresh_all();
    Ok(())
}

fn pump_events(ui: &Rc<Ui>, events: mpsc::Receiver<Event>) {
    let ui = Rc::clone(ui);
    glib::timeout_add_local(EVENT_PUMP, move || {
        loop {
            match events.try_recv() {
                Ok(event) => ui.handle_event(event),
                Err(mpsc::TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(mpsc::TryRecvError::Disconnected) => {
                    if ui.engine.borrow().is_some() {
                        tracing::warn!("engine event channel closed unexpectedly");
                    } else {
                        tracing::debug!("engine event channel closed");
                    }
                    return glib::ControlFlow::Break;
                }
            }
        }
    });
}

fn install_actions(app: &adw::Application, ui: &Rc<Ui>) {
    let simple = |name: &str, handler: Handler| {
        let action = gio::SimpleAction::new(name, None);
        let handler_ui = Rc::clone(ui);
        action.connect_activate(move |_, _| handler(&handler_ui));
        ui.window.add_action(&action);
    };
    simple(
        "reload",
        Box::new(|ui| ui.send(Command::ReadTemp, Job::ReadTemp)),
    );
    simple(
        "reconnect",
        Box::new(|ui| ui.send(Command::Detect, Job::Detect)),
    );
    simple("backup", Box::new(|ui| ui.backup_temp()));
    simple(
        "restore",
        Box::new(|ui| ui.send(Command::ListBackups, Job::ListBackups)),
    );
    simple("save-preset", Box::new(|ui| ui.prompt_save_preset(None)));
    simple(
        "search",
        Box::new(|ui| {
            ui.preset_search.grab_focus();
        }),
    );
    simple("about", Box::new(|ui| dialogs::about(&ui.window)));
    let save_channel = gio::SimpleAction::new("save-channel", None);
    save_channel.set_enabled(false);
    ui.window.add_action(&save_channel);
    let switch = gio::SimpleAction::new("switch-channel", Some(glib::VariantTy::STRING));
    let switch_ui = Rc::clone(ui);
    switch.connect_activate(move |_, parameter| {
        let channel = parameter
            .and_then(glib::Variant::get::<String>)
            .and_then(|text| text.parse::<Channel>().ok());
        if let Some(channel) = channel {
            switch_ui.request_switch(channel);
        }
    });
    ui.window.add_action(&switch);
    let apply = gio::SimpleAction::new("apply-preset", Some(glib::VariantTy::STRING));
    let apply_ui = Rc::clone(ui);
    apply.connect_activate(move |_, parameter| {
        let Some(key) = parameter.and_then(glib::Variant::get::<String>) else {
            return;
        };
        let entry = apply_ui
            .model
            .borrow()
            .presets
            .iter()
            .find(|entry| entry.key == key)
            .cloned();
        if let Some(entry) = entry {
            apply_ui.apply_preset(entry);
        }
    });
    ui.window.add_action(&apply);
    let undo_chain = gio::SimpleAction::new("undo-chain", Some(glib::VariantTy::STRING));
    let undo_ui = Rc::clone(ui);
    undo_chain.connect_activate(move |_, parameter| {
        let order = parameter
            .and_then(glib::Variant::get::<String>)
            .and_then(|text| parse_hex(&text));
        if let Some(order) = order
            && undo_ui.chain_editable()
        {
            undo_ui.write_chain(order, true);
        }
    });
    ui.window.add_action(&undo_chain);
    let quit = gio::SimpleAction::new("quit", None);
    let quit_ui = Rc::clone(ui);
    quit.connect_activate(move |_, _| quit_ui.window.close());
    app.add_action(&quit);
    app.set_accels_for_action("win.reload", &["<Control>r"]);
    app.set_accels_for_action("win.save-preset", &["<Control>s"]);
    app.set_accels_for_action("win.search", &["<Control>f"]);
    app.set_accels_for_action("app.quit", &["<Control>q"]);
    app.set_accels_for_action("window.close", &["<Control>w"]);
}

fn main_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let amp = gio::Menu::new();
    amp.append(Some(&tr("Reload From Amp")), Some("win.reload"));
    amp.append(Some(&tr("Reconnect")), Some("win.reconnect"));
    menu.append_section(None, &amp);
    let temp = gio::Menu::new();
    temp.append(Some(&tr("Back Up Temp Patch")), Some("win.backup"));
    temp.append(Some(&tr("Restore Backup to Temp…")), Some("win.restore"));
    temp.append(
        Some(&tr("Save Current Sound as Preset…")),
        Some("win.save-preset"),
    );
    menu.append_section(None, &temp);
    let channel = gio::Menu::new();
    channel.append(
        Some(&tr("Save Temp to Channel… (Not Implemented)")),
        Some("win.save-channel"),
    );
    menu.append_section(None, &channel);
    let about = gio::Menu::new();
    about.append(Some(&tr("About Katana Control")), Some("win.about"));
    menu.append_section(None, &about);
    menu
}

fn build_blocks_row() -> (gtk::Box, gtk::Box, gtk::Box, gtk::Box) {
    let (chain, chain_box) = node_strip(&tr("Signal Chain"));
    let (output, extras_box) = node_strip(&tr("Output"));
    (chain, chain_box, output, extras_box)
}

fn node_strip(heading: &str) -> (gtk::Box, gtk::Box) {
    let row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(NODE_GAP)
        .valign(gtk::Align::Start)
        .margin_start(ROW_INSET)
        .margin_end(ROW_INSET)
        .margin_top(ROW_INSET)
        .margin_bottom(ROW_INSET)
        .build();
    let wrap = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .halign(gtk::Align::Center)
        .margin_bottom(SCROLLBAR_ROOM)
        .build();
    let title = section_heading(heading);
    title.set_margin_start(ROW_INSET);
    wrap.append(&title);
    wrap.append(&flow_arrow());
    wrap.append(&row);
    let scroller = gtk::ScrolledWindow::builder()
        .vscrollbar_policy(gtk::PolicyType::Never)
        .hexpand(true)
        .vexpand(false)
        .child(&wrap)
        .build();
    let strip = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .build();
    strip.append(&scroller);
    (strip, row)
}

fn flow_arrow() -> gtk::DrawingArea {
    let area = gtk::DrawingArea::builder()
        .content_height(12)
        .margin_start(ROW_INSET)
        .margin_end(ROW_INSET)
        .margin_top(8)
        .css_classes(["flow-arrow"])
        .build();
    area.set_draw_func(|area, cr, width, height| {
        let color = area.color();
        let (width, height) = (f64::from(width), f64::from(height));
        let middle = (height / 2.0).floor() + 0.5;
        cr.set_source_rgba(
            f64::from(color.red()),
            f64::from(color.green()),
            f64::from(color.blue()),
            f64::from(color.alpha()) * 0.35,
        );
        cr.set_line_width(1.0);
        cr.set_line_cap(gtk::cairo::LineCap::Round);
        cr.set_line_join(gtk::cairo::LineJoin::Round);
        cr.move_to(0.5, middle);
        cr.line_to(width - 1.0, middle);
        cr.move_to(width - 7.0, middle - 4.5);
        cr.line_to(width - 1.0, middle);
        cr.line_to(width - 7.0, middle + 4.5);
        if let Err(error) = cr.stroke() {
            tracing::debug!(%error, "drawing the signal flow arrow");
        }
    });
    area
}

fn build_editor(
    channel_section: &adw::Clamp,
    chain: &gtk::Box,
    detail: &adw::Clamp,
    output: &gtk::Box,
) -> gtk::Box {
    let editor = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    editor.append(channel_section);
    editor.append(chain);
    editor.append(detail);
    editor.append(output);
    editor
}

struct Detail {
    clamp: adw::Clamp,
    panel: gtk::Box,
    holder: adw::Bin,
    icon: gtk::Image,
    title: gtk::Label,
    subtitle: gtk::Label,
    moves: gtk::Box,
    actions: gio::SimpleActionGroup,
}

fn build_detail() -> Detail {
    let icon = gtk::Image::builder()
        .pixel_size(DETAIL_ICON)
        .valign(gtk::Align::Center)
        .css_classes(["detail-icon"])
        .build();
    let title = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["title-2"])
        .build();
    let subtitle = gtk::Label::builder()
        .xalign(0.0)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .css_classes(["dim-label"])
        .build();
    let titles = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(2)
        .hexpand(true)
        .valign(gtk::Align::Center)
        .build();
    titles.append(&title);
    titles.append(&subtitle);
    let moves = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .valign(gtk::Align::Center)
        .build();
    for step in [Move::Left, Move::Right] {
        let icon_name = match step {
            Move::Left => "go-previous-symbolic",
            _ => "go-next-symbolic",
        };
        let button = gtk::Button::builder()
            .icon_name(icon_name)
            .action_name(format!("detail.{}", step.action()))
            .tooltip_text(tr(step.label()))
            .css_classes(["flat"])
            .build();
        button.update_property(&[gtk::accessible::Property::Label(&tr(step.label()))]);
        moves.append(&button);
    }
    let menu = gtk::MenuButton::builder()
        .icon_name("view-more-horizontal-symbolic")
        .menu_model(&move_menu("detail"))
        .css_classes(["flat"])
        .tooltip_text(tr("Move Block"))
        .build();
    menu.update_property(&[gtk::accessible::Property::Label(&tr("Move Block"))]);
    moves.append(&menu);
    let head = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .css_classes(["detail-head"])
        .build();
    head.append(&icon);
    head.append(&titles);
    head.append(&moves);
    let holder = adw::Bin::builder().vexpand(true).build();
    let panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .vexpand(true)
        .css_classes(["card", "detail-sheet"])
        .overflow(gtk::Overflow::Hidden)
        .build();
    panel.append(&head);
    panel.append(&holder);
    let actions = gio::SimpleActionGroup::new();
    panel.insert_action_group("detail", Some(&actions));
    let clamp = adw::Clamp::builder()
        .child(&panel)
        .vexpand(true)
        .margin_start(DETAIL_MARGIN)
        .margin_end(DETAIL_MARGIN)
        .build();
    Detail {
        clamp,
        panel,
        holder,
        icon,
        title,
        subtitle,
        moves,
        actions,
    }
}

fn move_menu(prefix: &str) -> gio::Menu {
    let model = gio::Menu::new();
    let section = gio::Menu::new();
    for step in Move::ALL {
        let item = gio::MenuItem::new(
            Some(&tr(step.label())),
            Some(&format!("{prefix}.{}", step.action())),
        );
        item.set_attribute_value("accel", Some(&step.accel().to_variant()));
        if matches!(step, Move::Start) {
            model.append_section(None, &section);
        }
        if matches!(step, Move::Left | Move::Right) {
            section.append_item(&item);
        } else {
            model.append_item(&item);
        }
    }
    model
}

fn refresh_node_state(card: &Card, selected: bool, can_edit: bool) {
    if card.actions.is_some() && !card.root.has_css_class("dragging") {
        let cursor = if can_edit { "grab" } else { "pointer" };
        card.root.set_cursor_from_name(Some(cursor));
    }
    if selected {
        card.root.add_css_class("selected");
    } else {
        card.root.remove_css_class("selected");
    }
}

fn mini_node(spec: &'static BlockSpec) -> gtk::Box {
    let title = tr(spec.title);
    let icon = gtk::Image::builder()
        .icon_name(spec.icon)
        .pixel_size(MINI_ICON)
        .hexpand(true)
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .build();
    let node = gtk::Box::builder()
        .css_classes(["chain-mini"])
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .tooltip_text(&title)
        .build();
    node.append(&icon);
    node.update_property(&[gtk::accessible::Property::Label(&title)]);
    node
}

const DRAG_ICON_MARGIN: u8 = 24;
const NODE_WIDTH: i32 = 104;
const NODE_HEIGHT: i32 = 96;
const NODE_GAP: i32 = 20;
const ROW_INSET: i32 = 12;
const NODE_ICON: i32 = 28;
const NODE_CHARS: i32 = 11;
const NODE_INSET: i32 = 10;
const POWER_INSET: i32 = 4;
const MINI_ICON: i32 = 16;
const DETAIL_ICON: i32 = 32;
const DETAIL_MARGIN: i32 = 12;
const SCROLLBAR_ROOM: i32 = 16;
const WINDOW_MIN_WIDTH: i32 = 960;
const WINDOW_MIN_HEIGHT: i32 = 600;

fn drag_icon(card: &gtk::Widget) -> Option<gtk::gdk::Paintable> {
    let bounds = card.compute_bounds(card)?;
    let (width, height) = (bounds.width(), bounds.height());
    let margin = f32::from(DRAG_ICON_MARGIN);
    let image = gtk::WidgetPaintable::new(Some(card)).current_image();
    let snapshot = gtk::Snapshot::new();
    snapshot.translate(&gtk::graphene::Point::new(margin, margin));
    let outline =
        gtk::gsk::RoundedRect::from_rect(gtk::graphene::Rect::new(0.0, 0.0, width, height), 12.0);
    snapshot.append_outset_shadow(
        &outline,
        &gtk::gdk::RGBA::new(0.0, 0.0, 0.0, 0.22),
        0.0,
        6.0,
        0.0,
        16.0,
    );
    snapshot.push_rounded_clip(&outline);
    image.snapshot(&snapshot, f64::from(width), f64::from(height));
    snapshot.pop();
    snapshot.to_paintable(Some(&gtk::graphene::Size::new(
        width + 2.0 * margin,
        height + 2.0 * margin,
    )))
}

fn section_heading(text: &str) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .margin_start(16)
        .margin_top(12)
        .css_classes(["heading"])
        .build()
}

impl Ui {
    fn build(app: &adw::Application, table: &'static ParamTable, engine: EngineHandle) -> Rc<Self> {
        let (header, title, status_label, light, content_toggle) = build_header();
        let (channel_section, channel_buttons) = build_channels();
        let (sidebar, preset_search, preset_list) = build_sidebar();
        let (chain, chain_box, output, extras_box) = build_blocks_row();
        let detail = build_detail();
        let editor = build_editor(&channel_section, &chain, &detail.clamp, &output);

        let retry_button = gtk::Button::builder()
            .label(tr("Try Again"))
            .action_name("win.reconnect")
            .halign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action"])
            .build();
        let status_page = adw::StatusPage::builder().child(&retry_button).build();
        let stack = gtk::Stack::builder()
            .transition_type(gtk::StackTransitionType::Crossfade)
            .build();
        stack.add_named(&status_page, Some("status"));
        stack.add_named(&editor, Some("editor"));

        let split = build_split(&sidebar, &header, &stack, &content_toggle);
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&split));
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(tr("Katana Control"))
            .default_width(1920)
            .default_height(1080)
            .width_request(WINDOW_MIN_WIDTH)
            .height_request(WINDOW_MIN_HEIGHT)
            .content(&toasts)
            .build();

        let ui = Rc::new(Self {
            table,
            window,
            toasts,
            title,
            status_active: Rc::new(Cell::new(false)),
            status_label,
            light,
            channel_buttons,
            stack,
            status_page,
            retry_button,
            chain_box,
            extras_box,
            params_holder: detail.holder,
            detail_icon: detail.icon,
            detail_title: detail.title,
            detail_subtitle: detail.subtitle,
            detail_moves: detail.moves,
            detail_clamp: detail.clamp.clone(),
            detail_actions: detail.actions,
            preset_list,
            preset_search,
            sidebar,
            engine: RefCell::new(Some(engine)),
            model: RefCell::new(Model {
                selected: "amp",
                ..Model::default()
            }),
            jobs: RefCell::new(VecDeque::new()),
            updating: Rc::new(Cell::new(false)),
            pending: RefCell::new(BTreeMap::new()),
            write_timers: RefCell::new(BTreeMap::new()),
            in_flight: RefCell::new(BTreeMap::new()),
            status_serial: Rc::new(Cell::new(0)),
            closing: Cell::new(false),
            cards: RefCell::new(Vec::new()),
            chain_keys: RefCell::new(Vec::new()),
            refocus: Cell::new(None),
            gaps: RefCell::new(Vec::new()),
            dragging: Cell::new(None),
            drop_gap: Cell::new(None),
            chain_toast: RefCell::new(None),
            params: RefCell::new(None),
            search_text: Rc::new(RefCell::new(String::new())),
        });
        ui.connect_sidebar();
        ui.connect_chain_drop();
        ui.connect_detail(&detail.panel);
        let close_ui = Rc::clone(&ui);
        ui.window
            .connect_close_request(move |_| close_ui.on_close_request());
        ui
    }

    fn connect_sidebar(self: &Rc<Self>) {
        let filter_ui = Rc::downgrade(self);
        self.preset_list.set_filter_func(move |row| {
            let Some(ui) = filter_ui.upgrade() else {
                return true;
            };
            let query = ui.search_text.borrow().to_lowercase();
            if query.is_empty() {
                return true;
            }
            let model = ui.model.borrow();
            usize::try_from(row.index())
                .ok()
                .and_then(|index| model.presets.get(index))
                .is_some_and(|entry| {
                    entry.name.to_lowercase().contains(&query)
                        || entry.patch_name.to_lowercase().contains(&query)
                })
        });
        let search_ui = Rc::clone(self);
        self.preset_search.connect_search_changed(move |entry| {
            *search_ui.search_text.borrow_mut() = entry.text().to_string();
            search_ui.preset_list.invalidate_filter();
        });
    }

    fn connect_detail(self: &Rc<Self>, panel: &gtk::Box) {
        let target: MoveTarget = Rc::new(|ui: &Ui| {
            let selected = ui.model.borrow().selected;
            blocks::spec(selected)
                .filter(|spec| blocks::is_movable(spec))
                .map(|spec| spec.key)
        });
        self.add_move_actions(&self.detail_actions, &target);
        panel.add_controller(self.move_keys(target));
    }

    fn add_move_actions(self: &Rc<Self>, actions: &gio::SimpleActionGroup, target: &MoveTarget) {
        for step in Move::ALL {
            let action = gio::SimpleAction::new(step.action(), None);
            let weak = Rc::downgrade(self);
            let target = Rc::clone(target);
            action.connect_activate(move |_, _| {
                let Some(ui) = weak.upgrade() else {
                    return;
                };
                if let Some(key) = target(ui.as_ref()) {
                    glib::idle_add_local_once(move || ui.move_block(key, step));
                }
            });
            actions.add_action(&action);
        }
    }

    fn move_keys(self: &Rc<Self>, target: MoveTarget) -> gtk::EventControllerKey {
        let keys = gtk::EventControllerKey::new();
        let weak = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, pressed, _, state| {
            if !state.contains(gtk::gdk::ModifierType::ALT_MASK) {
                return glib::Propagation::Proceed;
            }
            let Some(ui) = weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            match Move::from_key(pressed).zip(target(ui.as_ref())) {
                Some((step, key)) => {
                    ui.move_block(key, step);
                    glib::Propagation::Stop
                }
                None => glib::Propagation::Proceed,
            }
        });
        keys
    }

    fn connect_chain_drop(self: &Rc<Self>) {
        let target = gtk::DropTarget::new(glib::Type::STRING, gtk::gdk::DragAction::MOVE);
        let motion_ui = Rc::downgrade(self);
        target.connect_motion(move |_, x, _| {
            let Some(ui) = motion_ui.upgrade() else {
                return gtk::gdk::DragAction::empty();
            };
            if ui.dragging.get().is_none() {
                return gtk::gdk::DragAction::empty();
            }
            ui.show_drop_gap(ui.useful_drop_gap(x));
            gtk::gdk::DragAction::MOVE
        });
        let leave_ui = Rc::downgrade(self);
        target.connect_leave(move |_| {
            if let Some(ui) = leave_ui.upgrade() {
                ui.show_drop_gap(None);
            }
        });
        let drop_ui = Rc::downgrade(self);
        target.connect_drop(move |_, value, x, _| {
            let Some(ui) = drop_ui.upgrade() else {
                return false;
            };
            ui.show_drop_gap(None);
            let Some(key) = ui.dragging.get() else {
                return false;
            };
            if value.get::<String>().ok().as_deref() != Some(key) {
                return false;
            }
            let Some((from, gap)) = ui.chain_position(key).zip(ui.useful_drop_gap(x)) else {
                return false;
            };
            let target = if gap > from { gap - 1 } else { gap };
            glib::idle_add_local_once(move || ui.move_block_to(key, target));
            true
        });
        self.chain_box.add_controller(target);
    }

    fn chain_position(&self, key: &str) -> Option<usize> {
        self.chain_keys
            .borrow()
            .iter()
            .filter(|candidate| blocks::spec(candidate).is_some_and(blocks::reorderable))
            .position(|candidate| *candidate == key)
    }

    fn useful_drop_gap(&self, x: f64) -> Option<usize> {
        let key = self.dragging.get()?;
        let from = self.chain_position(key)?;
        let cards = self.cards.borrow();
        let gap = cards
            .iter()
            .filter(|card| blocks::spec(card.key).is_some_and(blocks::reorderable))
            .filter_map(|card| card.root.compute_bounds(&self.chain_box))
            .filter(|bounds| f64::from(bounds.x() + bounds.width() / 2.0) < x)
            .count();
        (gap != from && gap != from + 1).then_some(gap)
    }

    fn show_drop_gap(&self, gap: Option<usize>) {
        if self.drop_gap.replace(gap) == gap {
            return;
        }
        for (index, item) in self.gaps.borrow().iter().enumerate() {
            item.set_active(Some(index) == gap);
        }
    }

    fn connect_card_drag(self: &Rc<Self>, spec: &'static BlockSpec, root: &gtk::Overlay) {
        let source = gtk::DragSource::builder()
            .actions(gtk::gdk::DragAction::MOVE)
            .build();
        let hotspot = Rc::new(Cell::new((0, 0)));
        let prepare_ui = Rc::downgrade(self);
        let prepare_hotspot = Rc::clone(&hotspot);
        source.connect_prepare(move |_, x, y| {
            let ui = prepare_ui.upgrade()?;
            if !ui.chain_editable() {
                return None;
            }
            prepare_hotspot.set((round_to_i32(x), round_to_i32(y)));
            Some(gtk::gdk::ContentProvider::for_value(&spec.key.to_value()))
        });
        let begin_ui = Rc::downgrade(self);
        let begin_root = root.downgrade();
        source.connect_drag_begin(move |source, _| {
            let Some(root) = begin_root.upgrade() else {
                return;
            };
            let (x, y) = hotspot.get();
            if let Some(icon) = drag_icon(root.upcast_ref()) {
                source.set_icon(
                    Some(&icon),
                    x + i32::from(DRAG_ICON_MARGIN),
                    y + i32::from(DRAG_ICON_MARGIN),
                );
            }
            root.add_css_class("dragging");
            root.set_cursor_from_name(Some("grabbing"));
            if let Some(ui) = begin_ui.upgrade() {
                ui.dragging.set(Some(spec.key));
            }
        });
        let end_ui = Rc::downgrade(self);
        let end_root = root.downgrade();
        source.connect_drag_end(move |_, _, _| {
            if let Some(root) = end_root.upgrade() {
                root.remove_css_class("dragging");
                root.set_cursor_from_name(Some("grab"));
            }
            if let Some(ui) = end_ui.upgrade() {
                ui.dragging.set(None);
                ui.show_drop_gap(None);
            }
        });
        root.add_controller(source);
    }

    fn on_close_request(&self) -> glib::Propagation {
        if !self.pending.borrow().is_empty() || self.is_busy() {
            self.closing.set(true);
            self.flush_writes_now();
            self.show_status_now(&tr("Finishing before closing…"));
            return glib::Propagation::Stop;
        }
        glib::Propagation::Proceed
    }

    fn stop_engine(&self) {
        if let Some(handle) = self.engine.borrow_mut().take() {
            handle.shutdown();
        }
    }

    fn send(&self, command: Command, job: Job) {
        let result = self
            .engine
            .borrow()
            .as_ref()
            .map_or(Err(EngineError::Stopped), |handle| handle.send(command));
        match result {
            Ok(()) => {
                tracing::debug!(?job, "queued");
                let starts = self.jobs.borrow().is_empty();
                if starts {
                    self.begin_status(&job);
                }
                self.jobs.borrow_mut().push_back(job);
                self.refresh_sensitivity();
            }
            Err(error) => self.toast(&error.to_string()),
        }
    }

    fn is_busy(&self) -> bool {
        self.jobs.borrow().iter().any(Job::shows_status)
    }

    fn can_write(&self) -> bool {
        writable(&self.model.borrow())
    }

    fn toast(&self, text: &str) {
        let toast = adw::Toast::builder().title(text).use_markup(false).build();
        self.toasts.add_toast(toast);
    }

    fn begin_status(&self, job: &Job) {
        if !job.shows_status() {
            return;
        }
        let text = job.status_text();
        let serial = self.status_serial.get().wrapping_add(1);
        self.status_serial.set(serial);
        let label = self.status_label.downgrade();
        let current = Rc::clone(&self.status_serial);
        let active = Rc::clone(&self.status_active);
        glib::timeout_add_local_once(STATUS_DELAY, move || {
            if let Some(label) = label.upgrade()
                && current.get() == serial
            {
                active.set(true);
                label.set_text(&text);
            }
        });
    }

    fn show_status_now(&self, text: &str) {
        self.status_active.set(true);
        self.status_label.set_text(text);
    }

    fn end_status(&self) {
        self.status_serial
            .set(self.status_serial.get().wrapping_add(1));
        self.status_active.set(false);
        self.refresh_light();
    }

    fn show_stage(&self, stage: Stage) {
        let text = match stage {
            Stage::EditorMode => tr("Preparing amp…"),
            Stage::Writing { .. } => tr("Writing…"),
            Stage::Verifying { attempt: 1 } => tr("Verifying…"),
            Stage::Verifying { attempt } => trf(
                "Verifying again (attempt {attempt})…",
                &[("attempt", &attempt.to_string())],
            ),
            Stage::Rewriting { bytes } => trf(
                "Retrying: rewriting {bytes} bytes…",
                &[("bytes", &bytes.to_string())],
            ),
            Stage::WaitingForAmp { .. } => tr("Waiting for the amp to accept the switch…"),
            Stage::ProgramChange { attempt: 1, .. } => tr("Switching…"),
            Stage::ProgramChange { attempt, .. } => trf(
                "Retrying switch (attempt {attempt})…",
                &[("attempt", &attempt.to_string())],
            ),
            Stage::ConfirmingChannel { .. } => tr("Verifying channel…"),
            Stage::ReadingTemp => tr("Reading from Katana…"),
        };
        tracing::info!(?stage, "progress");
        self.show_status_now(&text);
    }

    fn handle_event(self: &Rc<Self>, event: Event) {
        let job = if event.is_terminal() {
            self.jobs.borrow_mut().pop_front()
        } else {
            None
        };
        let ended = job.is_some();
        match event {
            Event::Progress(stage) => self.show_stage(stage),
            Event::Unsolicited(packet) => self.apply_unsolicited(&packet),
            Event::AmpSwitched(switch) => self.follow_amp_switch(switch),
            Event::Detected(report) => self.on_detected(report),
            Event::TempRead(image) => self.set_image(image),
            Event::WriteVerified(outcome) => {
                if outcome.attempts > 1 {
                    self.toast(&tr("Write verified after a retry"));
                }
                tracing::info!(
                    attempts = outcome.attempts,
                    packets = outcome.packets,
                    "write verified"
                );
                self.adopt_image(outcome.image);
            }
            Event::ChainWritten(outcome) => self.on_chain_written(job.as_ref(), outcome),
            Event::ChannelSwitched(outcome) => {
                self.model.borrow_mut().channel = Some(outcome.channel);
                self.set_clean_image(outcome.temp);
                let text = if outcome.attempts > 1 {
                    trf(
                        "Switched to {channel} after {attempts} attempts",
                        &[
                            ("channel", outcome.channel.label()),
                            ("attempts", &outcome.attempts.to_string()),
                        ],
                    )
                } else {
                    trf(
                        "Switched to {channel}",
                        &[("channel", outcome.channel.label())],
                    )
                };
                self.toast(&text);
            }
            Event::BackupSaved { key, image } => {
                self.set_image(image);
                match job {
                    Some(Job::Backup {
                        then: Some(follow_up),
                    }) => self.run_follow_up(follow_up),
                    _ => self.toast(&trf("Temp backed up as {key}", &[("key", &key)])),
                }
            }
            Event::Restored { key, outcome } => {
                self.set_clean_image(outcome.image);
                self.toast(&trf("Restored {key} to temp", &[("key", &key)]));
            }
            Event::PresetSaved { name, .. } => self.on_preset_saved(&name),
            Event::PresetApplied { name, outcome, .. } => {
                self.set_clean_image(outcome.image);
                let text = if outcome.attempts > 1 {
                    trf(
                        "Applied “{name}” (verified after a retry)",
                        &[("name", &name)],
                    )
                } else {
                    trf("Applied “{name}”", &[("name", &name)])
                };
                self.toast(&text);
            }
            Event::Presets(entries) => {
                self.model.borrow_mut().presets = entries;
                self.rebuild_presets();
            }
            Event::Backups(mut keys) => {
                keys.reverse();
                if keys.is_empty() {
                    self.toast(&tr("No backups yet"));
                } else {
                    let ui = Rc::clone(self);
                    dialogs::choose_backup(&self.window, keys, move |key| {
                        ui.backup_then(FollowUp::Restore { key });
                    });
                }
            }
            Event::Failed(error) => self.on_failed(job, &error),
            Event::ShutDown => {}
        }
        if ended {
            self.after_job();
        }
    }

    fn after_job(self: &Rc<Self>) {
        if !self.jobs.borrow().contains(&Job::WriteParams) {
            self.in_flight.borrow_mut().clear();
        }
        let next = self.jobs.borrow().front().cloned();
        match next {
            Some(job) if job.shows_status() => self.begin_status(&job),
            _ => self.end_status(),
        }
        if !self.is_busy() {
            self.flush_writes();
        }
        self.refresh_all();
        if self.closing.get() && !self.is_busy() && self.pending.borrow().is_empty() {
            self.window.close();
        }
    }

    fn on_detected(self: &Rc<Self>, report: DetectReport) {
        let supported = report.device.is_supported();
        tracing::info!(model = %report.device.model_name(), identity = %report.device.identity.hex(), "detected");
        {
            let mut model = self.model.borrow_mut();
            model.device = Some(report.device);
            model.channel = report.current_channel;
            model.image = None;
            model.baseline = None;
            model.failed_detect = false;
        }
        self.drop_pending_writes("amp detected");
        if supported {
            self.send(Command::ReadTemp, Job::ReadTemp);
        }
    }

    fn on_failed(self: &Rc<Self>, job: Option<Job>, error: &EngineError) {
        tracing::warn!(?job, %error, "command failed");
        match (job, error) {
            (_, EngineError::NotFound { .. }) | (Some(Job::Detect), _) => {
                let mut model = self.model.borrow_mut();
                model.device = None;
                model.image = None;
                model.baseline = None;
                model.failed_detect = true;
            }
            (Some(Job::Switch(channel)), _) => {
                let toast = adw::Toast::builder()
                    .title(trf(
                        "Channel {channel} could not be selected",
                        &[("channel", channel.label())],
                    ))
                    .button_label(tr("Try Again"))
                    .action_name("win.switch-channel")
                    .action_target(&channel.label().to_variant())
                    .timeout(10)
                    .build();
                self.toasts.add_toast(toast);
                self.send(Command::Detect, Job::Detect);
            }
            (Some(Job::Backup { then: Some(_) }), _) => {
                self.toast(&trf(
                    "Could not back up the current sound, nothing was changed: {error}",
                    &[("error", &error.to_string())],
                ));
            }
            (Some(Job::WriteParams), _) => {
                self.drop_pending_writes("write failed");
                self.toast(&trf(
                    "Could not write the change: {error}",
                    &[("error", &error.to_string())],
                ));
                self.send(Command::ReadTemp, Job::ReadTemp);
            }
            (Some(Job::WriteChain { .. }), _) => {
                self.toast(&trf(
                    "Could not change the signal chain: {error}",
                    &[("error", &error.to_string())],
                ));
                self.send(Command::ReadTemp, Job::ReadTemp);
            }
            (Some(Job::SavePreset), _) => {
                self.model.borrow_mut().pending_switch = None;
                self.toast(&error.to_string());
            }
            _ => self.toast(&error.to_string()),
        }
    }

    fn on_preset_saved(&self, name: &str) {
        {
            let mut model = self.model.borrow_mut();
            let image = model.image.clone();
            model.baseline = image;
        }
        self.toast(&trf("Saved preset “{name}”", &[("name", name)]));
        self.send(Command::ListPresets, Job::ListPresets);
        let next = self.model.borrow_mut().pending_switch.take();
        if let Some(channel) = next {
            self.switch_now(channel);
        }
    }

    fn set_image(&self, image: PatchImage) {
        let mut model = self.model.borrow_mut();
        if model.baseline.is_none() {
            model.baseline = Some(image.clone());
        }
        model.image = Some(image);
    }

    fn profile(&self) -> Option<&'static katana_protocol::DeviceProfile> {
        self.model
            .borrow()
            .device
            .as_ref()
            .and_then(|device| device.profile)
    }

    fn chain_write_queued(&self) -> bool {
        self.jobs
            .borrow()
            .iter()
            .any(|job| matches!(job, Job::WriteChain { .. }))
    }

    fn adopt_image(&self, image: PatchImage) {
        let mut image = image;
        if self.chain_write_queued() {
            let wanted = self
                .model
                .borrow()
                .image
                .as_ref()
                .and_then(|current| blocks::chain_codes(self.table, current));
            if let (Some(profile), Some(order)) = (self.profile(), wanted)
                && let Ok(next) = profile.with_chain(&image, &order)
            {
                image = next;
            }
        }
        self.set_image(image);
    }

    fn on_chain_written(&self, job: Option<&Job>, outcome: ChainOutcome) {
        let undo = matches!(job, Some(Job::WriteChain { undo: true }));
        let normalised = outcome.is_normalised();
        let changed = outcome.packets > 0;
        let previous = hex(&outcome.previous);
        tracing::info!(
            requested = %hex(&outcome.requested),
            actual = %hex(&outcome.actual),
            normalised,
            "effect chain written"
        );
        self.adopt_image(outcome.image);
        if !changed {
            return;
        }
        let title = if normalised {
            tr("The amp adjusted the signal chain order")
        } else if undo {
            tr("Signal chain change undone")
        } else {
            tr("Signal chain changed")
        };
        self.window
            .upcast_ref::<gtk::Widget>()
            .announce(&title, gtk::AccessibleAnnouncementPriority::Medium);
        let mut builder = adw::Toast::builder().title(&title).use_markup(false);
        if !undo {
            builder = builder
                .button_label(tr("Undo"))
                .action_name("win.undo-chain")
                .action_target(&previous.to_variant());
        }
        let toast = builder.build();
        if let Some(old) = self.chain_toast.replace(Some(toast.clone())) {
            old.dismiss();
        }
        self.toasts.add_toast(toast);
    }

    fn chain_editable(&self) -> bool {
        self.can_write()
            && self.jobs.borrow().iter().all(|job| {
                matches!(
                    job,
                    Job::WriteChain { .. } | Job::WriteParams | Job::ListPresets | Job::ListBackups
                )
            })
    }

    fn move_block(self: &Rc<Self>, key: &'static str, step: Move) {
        let target = {
            let model = self.model.borrow();
            let Some(image) = model.image.as_ref() else {
                return;
            };
            let keys: Vec<&str> = blocks::chain_order(self.table, image)
                .iter()
                .filter(|spec| blocks::reorderable(spec))
                .map(|spec| spec.key)
                .collect();
            keys.iter()
                .position(|candidate| *candidate == key)
                .and_then(|from| step.target(from, keys.len()))
        };
        if let Some(target) = target {
            self.move_block_to(key, target);
        }
    }

    fn move_block_to(self: &Rc<Self>, key: &'static str, target: usize) {
        if !self.chain_editable() || !blocks::spec(key).is_some_and(blocks::is_movable) {
            return;
        }
        let order = {
            let model = self.model.borrow();
            let Some(image) = model.image.as_ref() else {
                return;
            };
            let current = blocks::chain_codes(self.table, image);
            match blocks::reorder(self.table, image, key, target) {
                Some(order) if Some(&order) != current.as_ref() => order,
                _ => return,
            }
        };
        tracing::info!(block = key, target, "moving block in the signal chain");
        self.refocus.set(Some(key));
        self.write_chain(order, false);
    }

    fn write_chain(self: &Rc<Self>, order: Vec<u8>, undo: bool) {
        let Some(profile) = self.profile() else {
            return;
        };
        if let Err(error) = profile.validate_chain(&order) {
            tracing::warn!(%error, "refusing chain write");
            self.toast(&error.to_string());
            return;
        }
        {
            let mut model = self.model.borrow_mut();
            if let Some(image) = model.image.as_mut()
                && let Ok(next) = profile.with_chain(image, &order)
            {
                *image = next;
            }
        }
        if let Some(old) = self.chain_toast.take() {
            old.dismiss();
        }
        self.flush_writes_now();
        self.send(Command::WriteChain(order), Job::WriteChain { undo });
        self.refresh_all();
    }

    fn set_clean_image(&self, image: PatchImage) {
        let mut model = self.model.borrow_mut();
        model.baseline = Some(image.clone());
        model.image = Some(image);
    }

    fn is_dirty(&self) -> bool {
        if !self.pending.borrow().is_empty() {
            return true;
        }
        let model = self.model.borrow();
        let profile = model.device.as_ref().and_then(|device| device.profile);
        match (&model.image, &model.baseline) {
            (Some(image), Some(baseline)) => {
                image.differing_offsets(baseline).iter().any(|offset| {
                    profile.is_none_or(|profile| !profile.verify_rules.is_amp_derived(*offset))
                })
            }
            _ => false,
        }
    }

    fn follow_amp_switch(self: &Rc<Self>, switch: AmpSwitch) {
        tracing::info!(
            channel = %switch.channel,
            from_dump = switch.from_dump,
            "following channel change made on the amp"
        );
        self.drop_pending_writes("channel changed on the amp");
        {
            let mut model = self.model.borrow_mut();
            model.channel = Some(switch.channel);
            model.baseline = Some(switch.temp.clone());
            model.image = Some(switch.temp);
        }
        self.refresh_all();
    }

    fn apply_unsolicited(self: &Rc<Self>, packet: &Dt1) {
        let profile = self
            .model
            .borrow()
            .device
            .as_ref()
            .and_then(|device| device.profile);
        let Some(profile) = profile else {
            return;
        };
        if packet.address() == profile.current_channel {
            if let Ok(channel) = parse_current_channel(packet.data()) {
                tracing::info!(%channel, "amp reports channel change");
                self.model.borrow_mut().channel = Some(channel);
            }
            self.refresh_header();
            return;
        }
        let Some(start) = packet.address().distance_from(profile.temp_patch) else {
            return;
        };
        let mut changed = false;
        if let Some(image) = self.model.borrow_mut().image.as_mut() {
            for (position, &value) in (start..).zip(packet.data()) {
                changed |= image.set(PatchOffset::from_linear(position), value).is_ok();
            }
        }
        if changed && !self.is_busy() {
            tracing::debug!(%packet, "temp changed on the amp");
            self.refresh_all();
        }
    }

    fn edit_param(self: &Rc<Self>, id: &'static str, edit: Edit) {
        if self.updating.get() || !self.can_write() {
            return;
        }
        match edit {
            Edit::Discrete(value) => {
                if self.stage_write(id, value) {
                    self.cancel_write_timer(id);
                    self.flush_writes();
                }
            }
            Edit::Continuous(value) => {
                if self.stage_write(id, value) {
                    self.schedule_write(id);
                }
            }
            Edit::Released => {
                if self.pending.borrow().contains_key(id) {
                    self.cancel_write_timer(id);
                    self.flush_writes();
                }
            }
        }
    }

    fn stage_write(&self, id: &'static str, value: u16) -> bool {
        if self.pending.borrow().get(id) == Some(&value) {
            return false;
        }
        let written = self.in_flight.borrow().get(id).copied().or_else(|| {
            self.model
                .borrow()
                .image
                .as_ref()
                .and_then(|image| blocks::value(self.table, image, id))
        });
        if written == Some(value) {
            if self.pending.borrow_mut().remove(id).is_some() {
                self.cancel_write_timer(id);
                self.refresh_header();
            }
            return false;
        }
        self.pending.borrow_mut().insert(id, value);
        self.refresh_header();
        true
    }

    fn schedule_write(self: &Rc<Self>, id: &'static str) {
        self.cancel_write_timer(id);
        let ui = Rc::downgrade(self);
        let source = glib::timeout_add_local_once(CONTINUOUS_WRITE_DEBOUNCE, move || {
            if let Some(ui) = ui.upgrade() {
                ui.write_timers.borrow_mut().remove(id);
                ui.flush_writes();
            }
        });
        self.write_timers.borrow_mut().insert(id, source);
    }

    fn cancel_write_timer(&self, id: &str) {
        if let Some(source) = self.write_timers.borrow_mut().remove(id) {
            source.remove();
        }
    }

    fn cancel_write_timers(&self) {
        let timers = std::mem::take(&mut *self.write_timers.borrow_mut());
        for source in timers.into_values() {
            source.remove();
        }
    }

    fn drop_pending_writes(&self, reason: &str) {
        self.cancel_write_timers();
        let dropped = std::mem::take(&mut *self.pending.borrow_mut());
        if !dropped.is_empty() {
            tracing::info!(?dropped, reason, "dropping unsent parameter changes");
        }
    }

    fn flush_writes_now(&self) {
        self.cancel_write_timers();
        if !self.can_write() {
            return;
        }
        let writes = self.take_pending_writes();
        if writes.is_empty() {
            return;
        }
        tracing::info!(?writes, "writing parameters before the next step");
        self.send(Command::WriteParams(writes), Job::WriteParams);
    }

    fn take_pending_writes(&self) -> Vec<ParamWrite> {
        let pending = std::mem::take(&mut *self.pending.borrow_mut());
        self.in_flight.borrow_mut().extend(pending.iter());
        pending
            .into_iter()
            .map(|(id, value)| ParamWrite {
                id: id.to_owned(),
                value,
            })
            .collect()
    }

    fn flush_writes(&self) {
        if self.is_busy() || !self.can_write() {
            return;
        }
        let writes = self.take_pending_writes();
        if writes.is_empty() {
            return;
        }
        tracing::info!(?writes, "writing parameters");
        self.send(Command::WriteParams(writes), Job::WriteParams);
    }

    fn timestamp() -> String {
        glib::DateTime::now_local()
            .and_then(|now| now.format("%Y%m%d-%H%M%S"))
            .map_or_else(|_| "now".to_owned(), |text| text.to_string())
    }

    fn backup_temp(&self) {
        self.flush_writes_now();
        let key = format!("temp-{}.syx", Self::timestamp());
        self.send(Command::BackupTemp { key }, Job::Backup { then: None });
    }

    fn backup_then(&self, follow_up: FollowUp) {
        self.flush_writes_now();
        let reason = match follow_up {
            FollowUp::ApplyPreset { .. } => "preset",
            FollowUp::Restore { .. } => "restore",
        };
        let key = format!("auto-before-{reason}-{}.syx", Self::timestamp());
        self.send(
            Command::BackupTemp { key },
            Job::Backup {
                then: Some(follow_up),
            },
        );
    }

    fn run_follow_up(&self, follow_up: FollowUp) {
        match follow_up {
            FollowUp::ApplyPreset { key, name } => {
                self.send(Command::ApplyPreset { key }, Job::ApplyPreset(name));
            }
            FollowUp::Restore { key } => self.send(Command::RestoreBackup { key }, Job::Restore),
        }
    }

    fn apply_preset(&self, entry: PresetEntry) {
        if !self.can_write() || self.is_busy() {
            return;
        }
        self.backup_then(FollowUp::ApplyPreset {
            key: entry.key,
            name: entry.name,
        });
    }

    fn prompt_save_preset(self: &Rc<Self>, then_switch: Option<Channel>) {
        if !self.can_write() {
            return;
        }
        let suggestion = self
            .model
            .borrow()
            .image
            .as_ref()
            .map(PatchImage::name)
            .unwrap_or_default();
        let ui = Rc::clone(self);
        dialogs::ask_preset_name(&self.window, &suggestion, move |name| {
            let exists = ui
                .model
                .borrow()
                .presets
                .iter()
                .any(|entry| entry.name.eq_ignore_ascii_case(&name));
            let save_ui = Rc::clone(&ui);
            let save_name = name.clone();
            let save = move || {
                save_ui.flush_writes_now();
                save_ui.model.borrow_mut().pending_switch = then_switch;
                save_ui.send(
                    Command::SavePreset {
                        name: save_name.clone(),
                    },
                    Job::SavePreset,
                );
            };
            if exists {
                dialogs::confirm_replace(&ui.window, &name, save);
            } else {
                save();
            }
        });
    }

    fn request_switch(self: &Rc<Self>, channel: Channel) {
        if !self.can_write() || self.is_busy() {
            return;
        }
        if !self.is_dirty() {
            tracing::info!(%channel, "temp unchanged, switching without asking");
            self.switch_now(channel);
            return;
        }
        tracing::info!(%channel, "temp edited, asking before switching");
        let ui = Rc::clone(self);
        dialogs::confirm_switch(&self.window, channel, move |choice| match choice {
            SwitchChoice::Switch => ui.switch_now(channel),
            SwitchChoice::SaveFirst => ui.prompt_save_preset(Some(channel)),
        });
    }

    fn switch_now(&self, channel: Channel) {
        self.drop_pending_writes("switching channel");
        self.send(Command::SwitchChannel(channel), Job::Switch(channel));
    }

    fn select_block(self: &Rc<Self>, key: &'static str) {
        if self.model.borrow().selected == key {
            return;
        }
        self.model.borrow_mut().selected = key;
        self.refresh_all();
    }

    fn refresh_all(self: &Rc<Self>) {
        self.refresh_stack();
        self.refresh_header();
        self.refresh_chain();
        self.refresh_params();
        self.refresh_sensitivity();
    }

    fn refresh_stack(&self) {
        let model = self.model.borrow();
        let searching = self.jobs.borrow().contains(&Job::Detect);
        let page = &self.status_page;
        page.set_paintable(None::<&gtk::gdk::Paintable>);
        self.retry_button.set_visible(false);
        match (&model.device, &model.image) {
            (Some(device), _) if !device.is_supported() => {
                page.set_icon_name(Some("dialog-warning-symbolic"));
                page.set_title(&tr("This Model Is Not Supported Yet"));
                page.set_description(Some(&trf(
                    "Katana Control currently supports only the MK II series.\nDetected identity: {identity}\nAll writes are disabled.",
                    &[("identity", &device.identity.hex())],
                )));
            }
            (Some(_), Some(_)) => {
                self.stack.set_visible_child_name("editor");
                return;
            }
            (Some(_), None) => {
                page.set_paintable(Some(&adw::SpinnerPaintable::new(Some(page))));
                page.set_title(&tr("Reading from Katana…"));
                page.set_description(None);
            }
            (None, _) if searching || !model.failed_detect => {
                page.set_paintable(Some(&adw::SpinnerPaintable::new(Some(page))));
                page.set_title(&tr("Looking for Katana…"));
                page.set_description(None);
            }
            (None, _) => {
                page.set_icon_name(Some("audio-speakers-symbolic"));
                page.set_title(&tr("Katana Not Connected"));
                page.set_description(Some(&tr("Connect the amp via USB and turn it on.")));
                self.retry_button.set_visible(true);
            }
        }
        self.stack.set_visible_child_name("status");
    }

    fn refresh_header(&self) {
        let dirty = self.is_dirty();
        let model = self.model.borrow();
        let patch = model
            .image
            .as_ref()
            .map(PatchImage::name)
            .filter(|name| !name.is_empty());
        let title = match patch {
            Some(name) if dirty => trf("{name} • Edited", &[("name", &name)]),
            Some(name) => name,
            None => tr("Katana Control"),
        };
        self.title.set_title(&title);
        let profile = model.device.as_ref().and_then(|device| device.profile);
        for (channel, button) in &self.channel_buttons {
            let active = model.channel == Some(*channel);
            if active {
                button.add_css_class("suggested-action");
            } else {
                button.remove_css_class("suggested-action");
            }
            let available = profile.is_some_and(|profile| profile.has_channel(*channel));
            let tooltip = if available {
                trf("Switch to {channel}", &[("channel", channel.label())])
            } else {
                tr("Not available on this amp")
            };
            button.set_tooltip_text(Some(&tooltip));
        }
    }

    fn refresh_sensitivity(&self) {
        let writable = self.can_write();
        let busy = self.is_busy();
        let profile = self
            .model
            .borrow()
            .device
            .as_ref()
            .and_then(|device| device.profile);
        for (channel, button) in &self.channel_buttons {
            let available = profile.is_some_and(|profile| profile.has_channel(*channel));
            button.set_sensitive(writable && available && !busy);
        }
        self.preset_list.set_sensitive(writable && !busy);
        for name in ["backup", "restore", "save-preset", "reload"] {
            if let Some(action) = self
                .window
                .lookup_action(name)
                .and_then(|action| action.downcast::<gio::SimpleAction>().ok())
            {
                action.set_enabled(writable && !busy);
            }
        }
        if let Some(action) = self
            .window
            .lookup_action("reconnect")
            .and_then(|action| action.downcast::<gio::SimpleAction>().ok())
        {
            action.set_enabled(!busy);
        }
        self.sidebar.set_sensitive(true);
        self.refresh_light();
        self.refresh_card_actions();
    }

    fn refresh_light(&self) {
        let busy = self.is_busy() || !self.pending.borrow().is_empty();
        let model = self.model.borrow();
        let connecting = self.jobs.borrow().contains(&Job::Detect);
        let (class, text) = match &model.device {
            None if connecting => ("conn-busy", tr("Looking for Katana…")),
            None => ("conn-off", tr("Not connected")),
            Some(device) => {
                let name = device.model_name();
                let text = if device.is_supported() {
                    trf("{model} connected", &[("model", &name)])
                } else {
                    trf("{model} connected, read-only", &[("model", &name)])
                };
                (if busy { "conn-busy" } else { "conn-ok" }, text)
            }
        };
        for name in ["conn-ok", "conn-busy", "conn-off"] {
            if name == class {
                self.light.add_css_class(name);
            } else {
                self.light.remove_css_class(name);
            }
        }
        let state = match class {
            "conn-ok" => tr("Connected"),
            "conn-busy" => tr("Busy"),
            _ => tr("Disconnected"),
        };
        self.light
            .update_property(&[gtk::accessible::Property::Label(&state)]);
        if !self.status_active.get() {
            self.status_label.set_text(&text);
        }
    }

    fn refresh_card_actions(&self) {
        let editable = self.chain_editable();
        let keys = self.chain_keys.borrow();
        let chain: Vec<&str> = keys
            .iter()
            .copied()
            .filter(|key| blocks::spec(key).is_some_and(blocks::reorderable))
            .collect();
        let enable = |actions: &gio::SimpleActionGroup, key: Option<&str>| {
            let from = key.and_then(|key| chain.iter().position(|candidate| *candidate == key));
            for step in Move::ALL {
                let enabled = editable
                    && from
                        .and_then(|from| step.target(from, chain.len()))
                        .is_some();
                if let Some(action) = actions
                    .lookup_action(step.action())
                    .and_then(|action| action.downcast::<gio::SimpleAction>().ok())
                {
                    action.set_enabled(enabled);
                }
            }
        };
        for card in self.cards.borrow().iter() {
            if let Some(actions) = &card.actions {
                enable(actions, Some(card.key));
            }
        }
        let selected = self.model.borrow().selected;
        let movable = blocks::spec(selected).is_some_and(blocks::is_movable);
        self.detail_moves.set_visible(movable);
        enable(&self.detail_actions, movable.then_some(selected));
    }

    fn rebuild_presets(&self) {
        self.preset_list.remove_all();
        let model = self.model.borrow();
        for entry in &model.presets {
            let subtitle = if entry.patch_name.is_empty() || entry.patch_name == entry.name {
                entry.model.clone()
            } else {
                entry.patch_name.clone()
            };
            let row = adw::ActionRow::builder()
                .title(&entry.name)
                .subtitle(subtitle)
                .use_markup(false)
                .activatable(true)
                .action_name("win.apply-preset")
                .action_target(&entry.key.to_variant())
                .tooltip_text(tr("Apply to temp and verify"))
                .build();
            row.add_css_class("preset-row");
            let apply = gtk::Button::builder()
                .icon_name("katana-upload-symbolic")
                .valign(gtk::Align::Center)
                .css_classes(["flat", "preset-apply"])
                .action_name("win.apply-preset")
                .action_target(&entry.key.to_variant())
                .tooltip_text(tr("Apply to temp and verify"))
                .build();
            apply.set_cursor_from_name(Some("pointer"));
            apply.update_property(&[gtk::accessible::Property::Label(&tr(
                "Apply to temp and verify",
            ))]);
            row.add_suffix(&apply);
            self.preset_list.append(&row);
        }
    }

    fn refresh_chain(self: &Rc<Self>) {
        let model = self.model.borrow();
        let Some(image) = model.image.as_ref() else {
            return;
        };
        let order: Vec<&'static BlockSpec> = blocks::chain_order(self.table, image)
            .into_iter()
            .chain(blocks::extras())
            .collect();
        let keys: Vec<&'static str> = order.iter().map(|spec| spec.key).collect();
        if *self.chain_keys.borrow() != keys {
            self.rebuild_chain(&order);
            *self.chain_keys.borrow_mut() = keys;
        }
        if let Some(key) = self.refocus.take()
            && let Some(card) = self.cards.borrow().iter().find(|card| card.key == key)
        {
            card.root.grab_focus();
        }
        let chain: Vec<&'static str> = order
            .iter()
            .filter(|spec| blocks::reorderable(spec))
            .map(|spec| spec.key)
            .collect();
        let count = chain.len().to_string();
        let can_edit = writable(&model);
        let previous = self.updating.replace(true);
        for card in self.cards.borrow().iter() {
            let Some(spec) = blocks::spec(card.key) else {
                continue;
            };
            let title = tr(spec.title);
            if let Some(position) = chain.iter().position(|key| *key == card.key) {
                let position = (position + 1).to_string();
                let text = if blocks::is_movable(spec) {
                    trf(
                        "Position {position} of {count} in the signal chain. Drag it, use its context menu, or press Alt+Left and Alt+Right to move it.",
                        &[("position", &position), ("count", &count)],
                    )
                } else {
                    trf(
                        "Position {position} of {count} in the signal chain. The amp stays in place, effects move around it.",
                        &[("position", &position), ("count", &count)],
                    )
                };
                card.root
                    .update_property(&[gtk::accessible::Property::Description(&text)]);
            }
            refresh_node_state(card, card.key == model.selected, can_edit);
            let on = blocks::is_on(self.table, image, spec);
            if on == Some(false) {
                card.root.add_css_class("block-off");
            } else {
                card.root.remove_css_class("block-off");
            }
            if let (Some(power), Some(on)) = (&card.power, on) {
                power.set_active(on);
                power.set_sensitive(can_edit);
                let tip = if on {
                    trf("Turn {block} Off", &[("block", &title)])
                } else {
                    trf("Turn {block} On", &[("block", &title)])
                };
                power.set_tooltip_text(Some(&tip));
            }
            let variant = blocks::variant(self.table, image, spec);
            for class in [
                "variant-green",
                "variant-red",
                "variant-yellow",
                "variant-none",
            ] {
                card.dot.remove_css_class(class);
            }
            card.dot
                .add_css_class(variant.map_or("variant-none", Variant::css_class));
            let variant_text =
                variant.map_or_else(|| tr("Unknown Variant"), |variant| tr(variant.title()));
            card.dot.set_tooltip_text(Some(&variant_text));
            card.dot
                .update_property(&[gtk::accessible::Property::Label(&variant_text)]);
            let type_text = blocks::type_label(self.table, image, spec).unwrap_or_default();
            card.type_label.set_text(&type_text);
            card.type_label.set_visible(!type_text.is_empty());
            let tooltip = if type_text.is_empty() {
                title.clone()
            } else {
                format!("{title}\n{type_text}")
            };
            card.root.set_tooltip_text(Some(&tooltip));
        }
        self.updating.set(previous);
        drop(model);
        self.fit_detail();
    }

    fn rebuild_chain(self: &Rc<Self>, order: &[&'static BlockSpec]) {
        for container in [&self.chain_box, &self.extras_box] {
            while let Some(child) = container.first_child() {
                container.remove(&child);
            }
        }
        let mut cards = Vec::new();
        let mut gaps = Vec::new();
        for spec in order {
            if spec.mini {
                self.chain_box.append(&mini_node(spec));
                continue;
            }
            let card = self.make_card(spec);
            if spec.chain_label.is_none() {
                self.extras_box.append(&card.root);
            } else {
                if blocks::reorderable(spec) {
                    let gap = Gap::new();
                    self.chain_box.append(&gap.root);
                    gaps.push(gap);
                }
                self.chain_box.append(&card.root);
            }
            cards.push(card);
        }
        let end = Gap::new();
        self.chain_box.append(&end.root);
        gaps.push(end);
        *self.cards.borrow_mut() = cards;
        *self.gaps.borrow_mut() = gaps;
        self.drop_gap.set(None);
        self.fit_detail();
    }

    fn fit_detail(&self) {
        if self.dragging.get().is_some() {
            return;
        }
        let (_, natural, _, _) = self.chain_box.measure(gtk::Orientation::Horizontal, -1);
        let width = (natural - 2 * ROW_INSET).max(0);
        if width > 0 && self.detail_clamp.maximum_size() != width {
            self.detail_clamp.set_maximum_size(width);
            self.detail_clamp.set_tightening_threshold(width);
        }
    }

    fn make_card(self: &Rc<Self>, spec: &'static BlockSpec) -> Card {
        let title = tr(spec.title);
        let icon = gtk::Image::builder()
            .icon_name(spec.icon)
            .pixel_size(NODE_ICON)
            .css_classes(["node-icon"])
            .build();
        let name = gtk::Label::builder()
            .label(&title)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(NODE_CHARS)
            .css_classes(["node-title"])
            .build();
        let type_label = gtk::Label::builder()
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(NODE_CHARS)
            .css_classes(["caption", "dim-label", "block-type"])
            .build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .valign(gtk::Align::Center)
            .margin_start(8)
            .margin_end(8)
            .build();
        content.append(&icon);
        content.append(&name);
        content.append(&type_label);
        let root = gtk::Overlay::builder()
            .child(&content)
            .width_request(NODE_WIDTH)
            .height_request(NODE_HEIGHT)
            .focusable(true)
            .accessible_role(gtk::AccessibleRole::Button)
            .css_classes(["card", "chain-node"])
            .overflow(gtk::Overflow::Hidden)
            .build();
        if let Some(class) = spec.category.css_class() {
            root.add_css_class(class);
        }
        root.set_cursor_from_name(Some("pointer"));
        root.update_property(&[gtk::accessible::Property::Label(&title)]);
        let dot = gtk::Box::builder()
            .css_classes(["node-variant", "variant-none"])
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .margin_top(NODE_INSET)
            .margin_start(NODE_INSET)
            .visible(spec.select_param.is_some())
            .build();
        root.add_overlay(&dot);
        let power = spec.on_param.map(|id| self.power_toggle(spec, id, &root));
        self.connect_node_select(spec, &root);
        let actions = blocks::is_movable(spec).then(|| {
            let actions = gio::SimpleActionGroup::new();
            let target: MoveTarget = Rc::new(move |_: &Ui| Some(spec.key));
            self.add_move_actions(&actions, &target);
            root.insert_action_group("card", Some(&actions));
            root.add_controller(self.move_keys(target));
            Self::connect_node_menu(&root);
            self.connect_card_drag(spec, &root);
            actions
        });
        Card {
            key: spec.key,
            root,
            actions,
            dot,
            type_label,
            power,
        }
    }

    fn power_toggle(
        self: &Rc<Self>,
        spec: &'static BlockSpec,
        id: &'static str,
        root: &gtk::Overlay,
    ) -> gtk::ToggleButton {
        let dot = gtk::Box::builder()
            .css_classes(["node-power-dot"])
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        let toggle = gtk::ToggleButton::builder()
            .child(&dot)
            .css_classes(["flat", "circular", "node-power"])
            .halign(gtk::Align::End)
            .valign(gtk::Align::Start)
            .margin_top(POWER_INSET)
            .margin_end(POWER_INSET)
            .build();
        toggle.update_property(&[gtk::accessible::Property::Label(&trf(
            "Enable {block}",
            &[("block", &tr(spec.title))],
        ))]);
        toggle.set_cursor_from_name(Some("pointer"));
        let weak = Rc::downgrade(self);
        toggle.connect_toggled(move |toggle| {
            if let Some(ui) = weak.upgrade()
                && !ui.updating.get()
            {
                ui.edit_param(id, Edit::Discrete(u16::from(toggle.is_active())));
            }
        });
        root.add_overlay(&toggle);
        toggle
    }

    fn connect_node_select(self: &Rc<Self>, spec: &'static BlockSpec, root: &gtk::Overlay) {
        let click = gtk::GestureClick::builder()
            .button(gtk::gdk::BUTTON_PRIMARY)
            .build();
        click.connect_pressed(|gesture, _, _, _| {
            if let Some(widget) = gesture.widget() {
                widget.set_state_flags(gtk::StateFlags::ACTIVE, false);
            }
        });
        click.connect_end(|gesture, _| {
            if let Some(widget) = gesture.widget() {
                widget.unset_state_flags(gtk::StateFlags::ACTIVE);
            }
        });
        let click_ui = Rc::downgrade(self);
        let click_root = root.downgrade();
        click.connect_released(move |gesture, _, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            if let Some(root) = click_root.upgrade() {
                root.grab_focus();
            }
            if let Some(ui) = click_ui.upgrade() {
                ui.select_block(spec.key);
            }
        });
        root.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        let key_ui = Rc::downgrade(self);
        keys.connect_key_pressed(move |_, pressed, _, state| {
            let plain = !state.intersects(
                gtk::gdk::ModifierType::ALT_MASK | gtk::gdk::ModifierType::CONTROL_MASK,
            );
            let activate = matches!(
                pressed,
                gtk::gdk::Key::Return
                    | gtk::gdk::Key::KP_Enter
                    | gtk::gdk::Key::space
                    | gtk::gdk::Key::KP_Space
            );
            match key_ui.upgrade() {
                Some(ui) if plain && activate => {
                    ui.select_block(spec.key);
                    glib::Propagation::Stop
                }
                _ => glib::Propagation::Proceed,
            }
        });
        root.add_controller(keys);
    }

    fn connect_node_menu(root: &gtk::Overlay) {
        let menu = gtk::MenuButton::builder()
            .menu_model(&move_menu("card"))
            .halign(gtk::Align::Fill)
            .valign(gtk::Align::Fill)
            .opacity(0.0)
            .can_target(false)
            .focusable(false)
            .css_classes(["node-menu"])
            .build();
        menu.update_state(&[gtk::accessible::State::Hidden(true)]);
        root.add_overlay(&menu);
        let context = gtk::GestureClick::builder()
            .button(gtk::gdk::BUTTON_SECONDARY)
            .build();
        let popup = menu.downgrade();
        context.connect_pressed(move |gesture, _, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            if let Some(menu) = popup.upgrade() {
                menu.popup();
            }
        });
        root.add_controller(context);
    }

    fn refresh_params(self: &Rc<Self>) {
        let model = self.model.borrow();
        let Some(image) = model.image.as_ref() else {
            return;
        };
        let Some(spec) = blocks::spec(model.selected) else {
            return;
        };
        self.detail_title.set_text(&tr(spec.title));
        self.detail_icon.set_icon_name(Some(spec.icon));
        let type_text = blocks::type_label(self.table, image, spec).unwrap_or_default();
        self.detail_subtitle.set_text(&type_text);
        self.detail_subtitle.set_visible(!type_text.is_empty());
        let structure = Structure {
            block: spec.key,
            type_value: blocks::type_value(self.table, image, spec),
            writable: writable(&model),
        };
        let current = self
            .params
            .borrow()
            .as_ref()
            .map(|view| view.structure.clone());
        if current.as_ref() == Some(&structure) {
            let pending = self.pending.borrow();
            if let Some(view) = self.params.borrow().as_ref() {
                view.update(self.table, image, &self.updating, &|id| {
                    pending.contains_key(id)
                });
            }
            return;
        }
        let weak = Rc::downgrade(self);
        let on_change: OnChange = Rc::new(move |id, edit| {
            if let Some(ui) = weak.upgrade() {
                ui.edit_param(id, edit);
            }
        });
        let view = params_view::build(
            self.table,
            image,
            spec,
            structure,
            &self.updating,
            &on_change,
        );
        self.params_holder.set_child(Some(&view.root));
        *self.params.borrow_mut() = Some(view);
    }
}

fn round_to_i32(value: f64) -> i32 {
    format!("{value:.0}").parse().unwrap_or_default()
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn parse_hex(text: &str) -> Option<Vec<u8>> {
    text.split_whitespace()
        .map(|word| u8::from_str_radix(word, 16).ok())
        .collect()
}

fn writable(model: &Model) -> bool {
    model.image.is_some() && model.device.as_ref().is_some_and(DeviceInfo::is_supported)
}

type Header = (
    adw::HeaderBar,
    adw::WindowTitle,
    gtk::Label,
    gtk::Box,
    gtk::Revealer,
);

fn sidebar_toggle() -> gtk::ToggleButton {
    gtk::ToggleButton::builder()
        .icon_name("sidebar-show-symbolic")
        .tooltip_text(tr("Show Presets"))
        .active(true)
        .build()
}

fn build_split(
    sidebar: &gtk::Box,
    header: &adw::HeaderBar,
    stack: &gtk::Stack,
    content_toggle: &gtk::Revealer,
) -> adw::OverlaySplitView {
    let (sidebar_header, sidebar_toggle) = build_sidebar_header();
    let sidebar_pane = adw::ToolbarView::new();
    sidebar_pane.add_top_bar(&sidebar_header);
    sidebar_pane.set_content(Some(sidebar));
    let content_pane = adw::ToolbarView::new();
    content_pane.add_top_bar(header);
    content_pane.set_content(Some(stack));
    let split = adw::OverlaySplitView::builder()
        .sidebar(&sidebar_pane)
        .content(&content_pane)
        .min_sidebar_width(240.0)
        .build();
    let toggles = [
        Some(sidebar_toggle),
        content_toggle
            .child()
            .and_then(|child| child.downcast::<gtk::ToggleButton>().ok()),
    ];
    for toggle in toggles.iter().flatten() {
        toggle
            .bind_property("active", &split, "show-sidebar")
            .bidirectional()
            .sync_create()
            .build();
    }
    split
        .bind_property("show-sidebar", content_toggle, "reveal-child")
        .invert_boolean()
        .sync_create()
        .build();
    split
}

fn build_sidebar_header() -> (adw::HeaderBar, gtk::ToggleButton) {
    let header = adw::HeaderBar::new();
    let toggle = sidebar_toggle();
    toggle.set_tooltip_text(Some(&tr("Hide Presets")));
    header.pack_start(&toggle);
    header.pack_start(
        &gtk::Label::builder()
            .label(tr("Katana Control"))
            .css_classes(["heading"])
            .build(),
    );
    header.set_title_widget(Some(&adw::Bin::new()));
    (header, toggle)
}

fn build_header() -> Header {
    let title = adw::WindowTitle::new(&tr("Katana Control"), "");
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&title));
    let sidebar_button = sidebar_toggle();
    let reveal = gtk::Revealer::builder()
        .transition_type(gtk::RevealerTransitionType::Crossfade)
        .transition_duration(TOGGLE_FADE)
        .child(&sidebar_button)
        .build();
    header.pack_start(&reveal);
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&main_menu())
        .primary(true)
        .tooltip_text(tr("Main Menu"))
        .build();
    header.pack_end(&menu_button);
    let light = gtk::Box::builder()
        .css_classes(["conn-light", "conn-off"])
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .accessible_role(gtk::AccessibleRole::Img)
        .build();
    let status_label = gtk::Label::builder()
        .label(tr("Not connected"))
        .css_classes(["caption", "dim-label", "status-label"])
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(36)
        .valign(gtk::Align::Center)
        .build();
    let status_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .margin_end(6)
        .valign(gtk::Align::Center)
        .build();
    status_box.append(&light);
    status_box.append(&status_label);
    header.pack_end(&status_box);

    (header, title, status_label, light, reveal)
}

fn build_channels() -> (adw::Clamp, Vec<(Channel, gtk::Button)>) {
    let channel_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .homogeneous(true)
        .css_classes(["linked", "channel-row"])
        .build();
    let mut channel_buttons = Vec::new();
    for channel in Channel::ALL {
        let button = gtk::Button::builder()
            .label(channel.label())
            .css_classes(["channel-button"])
            .action_name("win.switch-channel")
            .action_target(&channel.label().to_variant())
            .build();
        channel_row.append(&button);
        channel_buttons.push((channel, button));
    }
    let section = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_start(12)
        .margin_end(12)
        .build();
    section.append(
        &gtk::Label::builder()
            .label(tr("Channel"))
            .xalign(0.0)
            .css_classes(["heading"])
            .build(),
    );
    section.append(&channel_row);
    let clamp = adw::Clamp::builder()
        .maximum_size(params_view::CONTENT_WIDTH)
        .tightening_threshold(params_view::CONTENT_TIGHTENING)
        .margin_top(12)
        .margin_bottom(4)
        .child(&section)
        .build();
    (clamp, channel_buttons)
}

fn build_sidebar() -> (gtk::Box, gtk::SearchEntry, gtk::ListBox) {
    let preset_search = gtk::SearchEntry::builder()
        .placeholder_text(tr("Search Presets"))
        .build();
    let preset_list = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar", "preset-list"])
        .selection_mode(gtk::SelectionMode::None)
        .build();
    preset_list.set_placeholder(Some(
        &gtk::Label::builder()
            .label(tr("No presets yet"))
            .css_classes(["dim-label"])
            .margin_top(24)
            .build(),
    ));
    let save_button = gtk::Button::builder()
        .label(tr("Save Current Sound as Preset"))
        .action_name("win.save-preset")
        .css_classes(["suggested-action"])
        .build();
    let sidebar = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(8)
        .margin_top(8)
        .margin_bottom(12)
        .margin_start(8)
        .margin_end(8)
        .build();
    sidebar.append(&preset_search);
    sidebar.append(
        &gtk::ScrolledWindow::builder()
            .vexpand(true)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&preset_list)
            .build(),
    );
    sidebar.append(&save_button);

    (sidebar, preset_search, preset_list)
}
