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

type Handler = Box<dyn Fn(&Rc<Ui>)>;

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
    arrow: Option<gtk::Image>,
}

impl Gap {
    fn new(with_arrow: bool) -> Self {
        let indicator = gtk::Box::builder()
            .css_classes(["drop-indicator"])
            .visible(false)
            .build();
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .visible(with_arrow)
            .build();
        root.append(&indicator);
        let arrow = with_arrow.then(|| {
            let arrow = gtk::Image::builder()
                .icon_name("go-next-symbolic")
                .css_classes(["chain-arrow"])
                .margin_start(2)
                .margin_end(2)
                .build();
            root.append(&arrow);
            arrow
        });
        Self {
            root,
            indicator,
            arrow,
        }
    }

    fn set_active(&self, active: bool) {
        self.indicator.set_visible(active);
        match &self.arrow {
            Some(arrow) => arrow.set_visible(!active),
            None => self.root.set_visible(active),
        }
    }
}

struct Card {
    key: &'static str,
    root: gtk::Box,
    button: gtk::Button,
    menu: Option<gtk::MenuButton>,
    actions: Option<gio::SimpleActionGroup>,
    dot: gtk::Box,
    type_label: gtk::Label,
    switch: Option<gtk::Switch>,
}

pub struct Ui {
    table: &'static ParamTable,
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    title: adw::WindowTitle,
    status_box: gtk::Box,
    status_label: gtk::Label,
    channel_buttons: Vec<(Channel, gtk::Button)>,
    stack: gtk::Stack,
    status_page: adw::StatusPage,
    retry_button: gtk::Button,
    chain_box: gtk::Box,
    extras_box: gtk::Box,
    params_holder: adw::Bin,
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
    status_serial: Cell<u64>,
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

fn build_blocks_row() -> (gtk::Box, gtk::Box, gtk::Box) {
    let chain_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .margin_bottom(14)
        .valign(gtk::Align::Start)
        .css_classes(["chain-row"])
        .build();
    let chain_scroller = gtk::ScrolledWindow::builder()
        .vscrollbar_policy(gtk::PolicyType::Never)
        .hexpand(true)
        .vexpand(false)
        .child(&chain_box)
        .build();
    let extras_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(6)
        .margin_bottom(14)
        .css_classes(["chain-row"])
        .build();
    let chain_section = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .hexpand(true)
        .build();
    chain_section.append(&section_heading(&tr("Signal Chain")));
    chain_section.append(&chain_scroller);
    let extras_section = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .build();
    extras_section.append(&section_heading(&tr("Output")));
    extras_section.append(&extras_box);
    let blocks_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .build();
    blocks_row.append(&chain_section);
    blocks_row.append(
        &gtk::Separator::builder()
            .orientation(gtk::Orientation::Vertical)
            .margin_top(12)
            .margin_bottom(12)
            .build(),
    );
    blocks_row.append(&extras_section);
    (blocks_row, chain_box, extras_box)
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
        let (header, title, status_box, status_label, sidebar_button) = build_header();
        let (channel_section, channel_buttons) = build_channels();
        let (sidebar, preset_search, preset_list) = build_sidebar();
        let (blocks_row, chain_box, extras_box) = build_blocks_row();
        let params_holder = adw::Bin::builder().vexpand(true).build();
        let editor = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        editor.append(&channel_section);
        editor.append(&blocks_row);
        editor.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
        editor.append(&params_holder);

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

        let split = adw::OverlaySplitView::builder()
            .sidebar(&sidebar)
            .content(&stack)
            .min_sidebar_width(240.0)
            .build();
        sidebar_button
            .bind_property("active", &split, "show-sidebar")
            .bidirectional()
            .sync_create()
            .build();
        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&split));
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&toolbar));
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title(tr("Katana Control"))
            .default_width(1920)
            .default_height(1080)
            .content(&toasts)
            .build();

        let ui = Rc::new(Self {
            table,
            window,
            toasts,
            title,
            status_box,
            status_label,
            channel_buttons,
            stack,
            status_page,
            retry_button,
            chain_box,
            extras_box,
            params_holder,
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
            status_serial: Cell::new(0),
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
            .filter(|candidate| {
                blocks::spec(candidate).is_some_and(|spec| spec.chain_label.is_some())
            })
            .position(|candidate| *candidate == key)
    }

    fn useful_drop_gap(&self, x: f64) -> Option<usize> {
        let key = self.dragging.get()?;
        let from = self.chain_position(key)?;
        let cards = self.cards.borrow();
        let gap = cards
            .iter()
            .filter(|card| blocks::spec(card.key).is_some_and(|spec| spec.chain_label.is_some()))
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

    fn connect_card_drag(
        self: &Rc<Self>,
        spec: &'static BlockSpec,
        root: &gtk::Box,
        bar: &gtk::Box,
    ) {
        let source = gtk::DragSource::builder()
            .actions(gtk::gdk::DragAction::MOVE)
            .build();
        let hotspot = Rc::new(Cell::new((0, 0)));
        let prepare_ui = Rc::downgrade(self);
        let prepare_hotspot = Rc::clone(&hotspot);
        let prepare_root = root.downgrade();
        let prepare_bar = bar.downgrade();
        source.connect_prepare(move |_, x, y| {
            let ui = prepare_ui.upgrade()?;
            if !ui.chain_editable() {
                return None;
            }
            let (dx, dy) = prepare_bar
                .upgrade()
                .zip(prepare_root.upgrade())
                .and_then(|(bar, root)| bar.compute_bounds(&root))
                .map_or((0.0, 0.0), |bounds| {
                    (f64::from(bounds.x()), f64::from(bounds.y()))
                });
            let (x, y) = (x + dx, y + dy);
            prepare_hotspot.set((round_to_i32(x), round_to_i32(y)));
            Some(gtk::gdk::ContentProvider::for_value(&spec.key.to_value()))
        });
        let begin_ui = Rc::downgrade(self);
        let begin_root = root.downgrade();
        source.connect_drag_begin(move |source, _| {
            let Some(root) = begin_root.upgrade() else {
                return;
            };
            let icon = gtk::WidgetPaintable::new(Some(&root)).current_image();
            let (x, y) = hotspot.get();
            source.set_icon(Some(&icon), x, y);
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
                root.set_cursor(None);
            }
            if let Some(ui) = end_ui.upgrade() {
                ui.dragging.set(None);
                ui.show_drop_gap(None);
            }
        });
        bar.add_controller(source);
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
        self.status_label.set_text(&job.status_text());
        let serial = self.status_serial.get().wrapping_add(1);
        self.status_serial.set(serial);
        let weak = self.window.downgrade();
        let status_box = self.status_box.clone();
        let current = self.status_serial.clone();
        glib::timeout_add_local_once(STATUS_DELAY, move || {
            if weak.upgrade().is_some() && current.get() == serial {
                status_box.set_visible(true);
            }
        });
    }

    fn show_status_now(&self, text: &str) {
        self.status_label.set_text(text);
        self.status_box.set_visible(true);
    }

    fn end_status(&self) {
        self.status_serial
            .set(self.status_serial.get().wrapping_add(1));
        self.status_box.set_visible(false);
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
        self.status_label.set_text(&text);
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
        let status = match &model.device {
            None => tr("Not connected"),
            Some(device) if device.is_supported() => {
                trf("{model} connected", &[("model", &device.model_name())])
            }
            Some(device) => trf(
                "{model} connected, read-only",
                &[("model", &device.model_name())],
            ),
        };
        self.title.set_subtitle(&status);
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
        self.refresh_card_actions();
    }

    fn refresh_card_actions(&self) {
        let editable = self.chain_editable();
        let keys = self.chain_keys.borrow();
        let chain: Vec<&str> = keys
            .iter()
            .copied()
            .filter(|key| blocks::spec(key).is_some_and(|spec| spec.chain_label.is_some()))
            .collect();
        for card in self.cards.borrow().iter() {
            let Some(actions) = &card.actions else {
                continue;
            };
            let from = chain.iter().position(|key| *key == card.key);
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
            if let Some(menu) = &card.menu {
                menu.set_sensitive(editable);
            }
        }
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
            row.add_suffix(&gtk::Image::from_icon_name("go-next-symbolic"));
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
            card.button.grab_focus();
        }
        let chain_count = order
            .iter()
            .filter(|spec| spec.chain_label.is_some())
            .count();
        let previous = self.updating.replace(true);
        for card in self.cards.borrow().iter() {
            let Some(spec) = blocks::spec(card.key) else {
                continue;
            };
            if let Some(position) = order
                .iter()
                .filter(|spec| spec.chain_label.is_some())
                .position(|spec| spec.key == card.key)
            {
                let position = (position + 1).to_string();
                let count = chain_count.to_string();
                let text = if blocks::is_movable(spec) {
                    trf(
                        "Position {position} of {count} in the signal chain. Alt+Left and Alt+Right move it.",
                        &[("position", &position), ("count", &count)],
                    )
                } else {
                    trf(
                        "Position {position} of {count} in the signal chain. The amp stays in place, effects move around it.",
                        &[("position", &position), ("count", &count)],
                    )
                };
                card.button
                    .update_property(&[gtk::accessible::Property::Description(&text)]);
            }
            if card.key == model.selected {
                card.root.add_css_class("selected");
            } else {
                card.root.remove_css_class("selected");
            }
            let on = blocks::is_on(self.table, image, spec);
            if on == Some(false) {
                card.root.add_css_class("block-off");
            } else {
                card.root.remove_css_class("block-off");
            }
            if let (Some(switch), Some(on)) = (&card.switch, on) {
                switch.set_active(on);
                switch.set_sensitive(writable(&model));
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
            card.type_label.set_tooltip_text(Some(&type_text));
        }
        self.updating.set(previous);
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
            let card = self.make_card(spec);
            if spec.chain_label.is_none() {
                self.extras_box.append(&card.root);
            } else {
                let gap = Gap::new(!gaps.is_empty());
                self.chain_box.append(&gap.root);
                gaps.push(gap);
                self.chain_box.append(&card.root);
            }
            cards.push(card);
        }
        let end = Gap::new(false);
        self.chain_box.append(&end.root);
        gaps.push(end);
        *self.cards.borrow_mut() = cards;
        *self.gaps.borrow_mut() = gaps;
        self.drop_gap.set(None);
    }

    fn make_card(self: &Rc<Self>, spec: &'static BlockSpec) -> Card {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .css_classes(["card", "chain-block"])
            .overflow(gtk::Overflow::Hidden)
            .vexpand(false)
            .build();
        let body = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .vexpand(true)
            .css_classes(["chain-body"])
            .build();
        root.append(&body);
        let dot = gtk::Box::builder()
            .css_classes(["variant-dot", "variant-none"])
            .valign(gtk::Align::Center)
            .visible(spec.select_param.is_some())
            .build();
        let title = gtk::Label::builder()
            .label(tr(spec.title))
            .xalign(0.0)
            .css_classes(["heading", "block-title"])
            .build();
        let top = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .build();
        top.append(&dot);
        top.append(&title);
        let type_label = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .max_width_chars(14)
            .width_chars(10)
            .css_classes(["caption", "dim-label", "block-type"])
            .build();
        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(2)
            .build();
        content.append(&top);
        content.append(&type_label);
        let button = gtk::Button::builder()
            .child(&content)
            .css_classes(["flat"])
            .tooltip_text(trf("Edit {block}", &[("block", &tr(spec.title))]))
            .build();
        let select_ui = Rc::clone(self);
        button.connect_clicked(move |_| select_ui.select_block(spec.key));
        body.append(&button);
        let switch = spec.on_param.map(|id| {
            let switch = gtk::Switch::builder()
                .halign(gtk::Align::Center)
                .valign(gtk::Align::Center)
                .build();
            switch.update_property(&[gtk::accessible::Property::Label(&trf(
                "Enable {block}",
                &[("block", &tr(spec.title))],
            ))]);
            let write_ui = Rc::clone(self);
            switch.connect_active_notify(move |switch| {
                if !write_ui.updating.get() {
                    write_ui.edit_param(id, Edit::Discrete(u16::from(switch.is_active())));
                }
            });
            body.append(&switch);
            switch
        });
        let (menu, actions) = if blocks::is_movable(spec) {
            let (menu, actions) = self.card_menu(spec, &root);
            let bar = Self::grab_bar(spec, &menu);
            self.connect_card_drag(spec, &root, &bar);
            root.append(&bar);
            (Some(menu), Some(actions))
        } else {
            (None, None)
        };
        Card {
            key: spec.key,
            root,
            button,
            menu,
            actions,
            dot,
            type_label,
            switch,
        }
    }

    fn grab_bar(spec: &'static BlockSpec, menu: &gtk::MenuButton) -> gtk::Box {
        let title = tr(spec.title);
        let bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .css_classes(["chain-grab"])
            .tooltip_text(trf("Drag to move {block}", &[("block", &title)]))
            .build();
        bar.set_cursor_from_name(Some("grab"));
        bar.update_property(&[gtk::accessible::Property::Label(&trf(
            "Move {block}",
            &[("block", &title)],
        ))]);
        let center = gtk::CenterBox::builder().hexpand(true).build();
        let grip = gtk::Image::builder()
            .icon_name("list-drag-handle-symbolic")
            .css_classes(["chain-grip"])
            .valign(gtk::Align::Center)
            .build();
        center.set_center_widget(Some(&grip));
        center.set_end_widget(Some(menu));
        bar.append(&center);
        bar
    }

    fn card_menu(
        self: &Rc<Self>,
        spec: &'static BlockSpec,
        root: &gtk::Box,
    ) -> (gtk::MenuButton, gio::SimpleActionGroup) {
        let actions = gio::SimpleActionGroup::new();
        let model = gio::Menu::new();
        let section = gio::Menu::new();
        for step in Move::ALL {
            let action = gio::SimpleAction::new(step.action(), None);
            let move_ui = Rc::clone(self);
            action.connect_activate(move |_, _| move_ui.move_block(spec.key, step));
            actions.add_action(&action);
            let item = gio::MenuItem::new(
                Some(&tr(step.label())),
                Some(&format!("card.{}", step.action())),
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
        root.insert_action_group("card", Some(&actions));
        let title = tr(spec.title);
        let menu = gtk::MenuButton::builder()
            .icon_name("view-more-horizontal-symbolic")
            .menu_model(&model)
            .css_classes(["flat", "circular", "chain-more"])
            .valign(gtk::Align::Center)
            .tooltip_text(trf("Move {block}", &[("block", &title)]))
            .build();
        menu.update_property(&[gtk::accessible::Property::Label(&trf(
            "Move {block}",
            &[("block", &title)],
        ))]);
        let context = gtk::GestureClick::builder()
            .button(gtk::gdk::BUTTON_SECONDARY)
            .build();
        let popup = menu.clone();
        context.connect_pressed(move |gesture, _, _, _| {
            gesture.set_state(gtk::EventSequenceState::Claimed);
            popup.popup();
        });
        root.add_controller(context);
        let keys = gtk::EventControllerKey::new();
        let key_ui = Rc::clone(self);
        keys.connect_key_pressed(move |_, key, _, state| {
            if !state.contains(gtk::gdk::ModifierType::ALT_MASK) {
                return glib::Propagation::Proceed;
            }
            match Move::from_key(key) {
                Some(step) => {
                    key_ui.move_block(spec.key, step);
                    glib::Propagation::Stop
                }
                None => glib::Propagation::Proceed,
            }
        });
        root.add_controller(keys);
        (menu, actions)
    }

    fn refresh_params(self: &Rc<Self>) {
        let model = self.model.borrow();
        let Some(image) = model.image.as_ref() else {
            return;
        };
        let Some(spec) = blocks::spec(model.selected) else {
            return;
        };
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
    gtk::Box,
    gtk::Label,
    gtk::ToggleButton,
);

fn build_header() -> Header {
    let title = adw::WindowTitle::new(&tr("Katana Control"), &tr("Not connected"));
    let header = adw::HeaderBar::new();
    header.set_title_widget(Some(&title));
    let sidebar_button = gtk::ToggleButton::builder()
        .icon_name("sidebar-show-symbolic")
        .tooltip_text(tr("Show Presets"))
        .active(true)
        .build();
    header.pack_start(&sidebar_button);
    let menu_button = gtk::MenuButton::builder()
        .icon_name("open-menu-symbolic")
        .menu_model(&main_menu())
        .primary(true)
        .tooltip_text(tr("Main Menu"))
        .build();
    header.pack_end(&menu_button);
    let status_label = gtk::Label::builder()
        .css_classes(["dim-label", "status-label"])
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(36)
        .build();
    let status_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(8)
        .visible(false)
        .build();
    status_box.append(&adw::Spinner::new());
    status_box.append(&status_label);
    header.pack_end(&status_box);

    (header, title, status_box, status_label, sidebar_button)
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
        .css_classes(["navigation-sidebar"])
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
    sidebar.append(
        &gtk::Label::builder()
            .label(tr("My Presets"))
            .xalign(0.0)
            .margin_start(6)
            .css_classes(["heading"])
            .build(),
    );
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
