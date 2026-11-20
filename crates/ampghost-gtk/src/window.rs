use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use adw::prelude::*;
use ampghost_engine::{
    AmpSwitch, ChainOutcome, Command, DetectReport, DeviceInfo, EngineError, EngineHandle, Event,
    ParamWrite, PresetEntry, Stage, WriteOutcome, apply_param_writes,
};
use ampghost_params::ParamTable;
use ampghost_protocol::{
    Channel, DeviceProfile, Dt1, KATANA_50_MK2, PatchImage, PatchOffset, parse_current_channel,
};
use ampghost_store::{FullBackup, TSL_MAX_PATCHES};
use anyhow::{Context, Result};
use gtk::{gio, glib};

use crate::blocks::{self, BlockSpec, Variant};
use crate::bridge;
use crate::dialogs::{self, SwitchChoice, ToneTarget};
use crate::files::{self, ToneFile};
use crate::i18n::{tr, trf};
use crate::params_view::{self, Edit, OnChange, ParamsView, Structure};

const EVENT_PUMP: Duration = Duration::from_millis(30);
const CONTINUOUS_WRITE_DEBOUNCE: Duration = Duration::from_millis(500);
const STATUS_DELAY: Duration = Duration::from_millis(300);
const TOGGLE_FADE: u32 = 250;

type Handler = Box<dyn Fn(&Rc<Ui>)>;

#[derive(Debug, Clone, PartialEq, Eq)]
enum FollowUp {
    ApplyPreset { key: String, name: String },
    PutBack(PatchImage),
    SendTone { image: PatchImage, name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Job {
    Detect,
    ReadTemp,
    WriteParams,
    WriteChain { undo: bool },
    Switch(Channel),
    Backup(FollowUp),
    SavePreset,
    UpdatePreset(String),
    DeletePreset(String),
    ApplyPreset(String),
    PutBack,
    ListPresets,
    BackupAmp(PathBuf),
    SaveTone(String),
    SendTone(String),
    ExportPresets(PathBuf),
}

impl Job {
    fn shows_status(&self) -> bool {
        !matches!(self, Self::ListPresets)
    }

    fn status_text(&self) -> String {
        match self {
            Self::Detect => tr("Looking for Katana…"),
            Self::ReadTemp => tr("Reading from Katana…"),
            Self::WriteParams => tr("Writing…"),
            Self::WriteChain { undo: false } => tr("Changing signal chain…"),
            Self::WriteChain { undo: true } => tr("Undoing chain change…"),
            Self::Switch(channel) => {
                trf("Switching to {channel}…", &[("channel", channel.label())])
            }
            Self::Backup(_) => tr("Backing up current sound…"),
            Self::SavePreset => tr("Saving preset…"),
            Self::UpdatePreset(name) => trf("Updating “{name}”…", &[("name", name)]),
            Self::DeletePreset(name) => trf("Deleting “{name}”…", &[("name", name)]),
            Self::ApplyPreset(name) => trf("Applying “{name}”…", &[("name", name)]),
            Self::PutBack => tr("Putting your edits back…"),
            Self::BackupAmp(_) => tr("Backing up amp…"),
            Self::SaveTone(name) => trf("Saving “{name}”…", &[("name", name)]),
            Self::SendTone(name) => trf("Sending “{name}” to temp…", &[("name", name)]),
            Self::ExportPresets(_) => tr("Exporting presets…"),
            Self::ListPresets => String::new(),
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
    chosen_preset: Option<String>,
    selected: &'static str,
    pending_switch: Option<Channel>,
    failed_detect: bool,
    held: Option<Held>,
    lost_edits: Option<PatchImage>,
}

struct Held {
    image: PatchImage,
    baseline: Option<PatchImage>,
}

struct Card {
    key: &'static str,
    root: gtk::Overlay,
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
    chain_picker: gtk::DropDown,
    extras_box: gtk::Box,
    params_holder: adw::Bin,
    detail_icon: gtk::Image,
    detail_title: gtk::Label,
    detail_subtitle: gtk::Label,
    detail_clamp: adw::Clamp,
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
    chain_toast: RefCell<Option<adw::Toast>>,
    params: RefCell<Option<ParamsView>>,
    search_text: Rc<RefCell<String>>,
}

pub fn present(app: &adw::Application) -> Result<()> {
    let table = ampghost_params::table().context("loading the parameter table")?;
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
    simple("put-back-edits", Box::new(|ui| ui.put_back_edits()));
    simple("backup-amp", Box::new(Ui::choose_backup_file));
    simple("import-tone", Box::new(Ui::choose_tone_file));
    simple("export-presets", Box::new(Ui::prompt_export));
    simple("save-preset", Box::new(|ui| ui.prompt_save_preset(None)));
    simple("update-preset", Box::new(Ui::prompt_update_preset));
    simple("delete-preset", Box::new(Ui::prompt_delete_preset));
    simple(
        "search",
        Box::new(|ui| {
            if let Some(split) = ui
                .preset_search
                .ancestor(adw::OverlaySplitView::static_type())
                .and_then(|widget| widget.downcast::<adw::OverlaySplitView>().ok())
            {
                split.set_show_sidebar(true);
            }
            ui.preset_search.grab_focus();
        }),
    );
    simple("about", Box::new(|ui| dialogs::about(&ui.window)));
    simple("shortcuts", Box::new(|ui| dialogs::shortcuts(&ui.window)));
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
    let undo_chain = gio::SimpleAction::new("undo-chain", Some(glib::VariantTy::BYTE));
    let undo_ui = Rc::clone(ui);
    undo_chain.connect_activate(move |_, parameter| {
        if let Some(pattern) = parameter.and_then(glib::Variant::get::<u8>)
            && undo_ui.chain_editable()
        {
            undo_ui.write_chain(pattern, true);
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
    app.set_accels_for_action("win.shortcuts", &["<Control>question"]);
}

fn main_menu() -> gio::Menu {
    let menu = gio::Menu::new();
    let amp = gio::Menu::new();
    amp.append(Some(&tr("Reload From Amp")), Some("win.reload"));
    amp.append(Some(&tr("Reconnect")), Some("win.reconnect"));
    menu.append_section(None, &amp);
    let files = gio::Menu::new();
    files.append(Some(&tr("Back Up Amp…")), Some("win.backup-amp"));
    files.append(Some(&tr("Import Presets (.tsl)…")), Some("win.import-tone"));
    files.append(
        Some(&tr("Export Presets (.tsl)…")),
        Some("win.export-presets"),
    );
    menu.append_section(None, &files);
    let about = gio::Menu::new();
    about.append(Some(&tr("Keyboard Shortcuts")), Some("win.shortcuts"));
    about.append(Some(&tr("About AmpGhost")), Some("win.about"));
    menu.append_section(None, &about);
    menu
}

fn build_blocks_row(chain_picker: &gtk::DropDown) -> (gtk::Box, gtk::Box, gtk::Box, gtk::Box) {
    let (chain, chain_box) = node_strip(&tr("Signal Chain"), Some(chain_picker.upcast_ref()));
    let (output, extras_box) = node_strip(&tr("Output"), None);
    (chain, chain_box, output, extras_box)
}

fn chain_picker(table: &ParamTable) -> gtk::DropDown {
    let labels = KATANA_50_MK2
        .chain_patterns
        .iter()
        .map(|pattern| blocks::pattern_label(table, pattern))
        .collect::<Vec<_>>();
    let names = labels.iter().map(String::as_str).collect::<Vec<_>>();
    let picker = gtk::DropDown::from_strings(&names);
    picker.set_halign(gtk::Align::End);
    picker.set_margin_end(ROW_INSET);
    picker.set_tooltip_text(Some(&tr("Signal chain order")));
    picker.update_property(&[gtk::accessible::Property::Label(&tr("Signal Chain Order"))]);
    picker.add_css_class("flat");
    picker
}

fn node_strip(heading: &str, accessory: Option<&gtk::Widget>) -> (gtk::Box, gtk::Box) {
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
    title.set_margin_top(6);
    title.set_hexpand(true);
    title.set_valign(gtk::Align::BaselineCenter);
    let heading_row = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .baseline_position(gtk::BaselinePosition::Center)
        .margin_top(6)
        .build();
    heading_row.append(&title);
    if let Some(accessory) = accessory {
        accessory.set_valign(gtk::Align::BaselineCenter);
        heading_row.append(accessory);
    }
    wrap.append(&heading_row);
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
    holder: adw::Bin,
    icon: gtk::Image,
    title: gtk::Label,
    subtitle: gtk::Label,
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
    let head = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .spacing(12)
        .css_classes(["detail-head"])
        .build();
    head.append(&icon);
    head.append(&titles);
    let holder = adw::Bin::builder().vexpand(true).build();
    let panel = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .vexpand(true)
        .css_classes(["card", "detail-sheet"])
        .overflow(gtk::Overflow::Hidden)
        .build();
    panel.append(&head);
    panel.append(&holder);
    let clamp = adw::Clamp::builder()
        .child(&panel)
        .vexpand(true)
        .margin_start(DETAIL_MARGIN)
        .margin_end(DETAIL_MARGIN)
        .build();
    Detail {
        clamp,
        holder,
        icon,
        title,
        subtitle,
    }
}

fn refresh_node_state(card: &Card, selected: bool) {
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
        let chain_picker = chain_picker(table);
        let (chain, chain_box, output, extras_box) = build_blocks_row(&chain_picker);
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
            .title(tr("AmpGhost"))
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
            chain_picker,
            extras_box,
            params_holder: detail.holder,
            detail_icon: detail.icon,
            detail_title: detail.title,
            detail_subtitle: detail.subtitle,
            detail_clamp: detail.clamp.clone(),
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
            chain_toast: RefCell::new(None),
            params: RefCell::new(None),
            search_text: Rc::new(RefCell::new(String::new())),
        });
        ui.connect_sidebar();
        ui.connect_chain_picker();
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
        let select_ui = Rc::downgrade(self);
        self.preset_list.connect_row_selected(move |_, row| {
            let Some(ui) = select_ui.upgrade() else {
                return;
            };
            let key = {
                let model = ui.model.borrow();
                row.and_then(|row| usize::try_from(row.index()).ok())
                    .and_then(|index| model.presets.get(index))
                    .map(|entry| entry.key.clone())
            };
            if row.is_some() || ui.model.borrow().presets.is_empty() {
                ui.model.borrow_mut().chosen_preset = key;
            }
            ui.refresh_sensitivity();
        });
        let search_ui = Rc::clone(self);
        self.preset_search.connect_search_changed(move |entry| {
            *search_ui.search_text.borrow_mut() = entry.text().to_string();
            search_ui.preset_list.invalidate_filter();
        });
    }

    fn connect_chain_picker(self: &Rc<Self>) {
        let weak = Rc::downgrade(self);
        self.chain_picker.connect_selected_notify(move |picker| {
            let Some(ui) = weak.upgrade() else {
                return;
            };
            if ui.updating.get() {
                return;
            }
            let Ok(pattern) = u8::try_from(picker.selected()) else {
                return;
            };
            let current = ui.current_pattern();
            if current == Some(pattern) || !ui.chain_editable() {
                ui.refresh_chain_picker();
                return;
            }
            glib::idle_add_local_once(move || ui.write_chain(pattern, false));
        });
    }

    fn current_pattern(&self) -> Option<u8> {
        let profile = self.profile()?;
        let model = self.model.borrow();
        profile.chain_pattern_of(model.image.as_ref()?)
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
            Stage::ReadingBackup { done, total } => trf(
                "Backing up amp… {done} of {total}",
                &[("done", &done.to_string()), ("total", &total.to_string())],
            ),
            Stage::Reloading { channel } => trf(
                "Reloading {channel} from the amp…",
                &[("channel", channel.label())],
            ),
            Stage::ReadingStored {
                channel,
                done,
                total,
            } => trf(
                "Reloading {channel} from the amp… {done} of {total}",
                &[
                    ("channel", channel.label()),
                    ("done", &done.to_string()),
                    ("total", &total.to_string()),
                ],
            ),
            Stage::Bouncing { channel } => trf(
                "Reloading {channel} by switching away and back…",
                &[("channel", channel.label())],
            ),
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
            Event::Reconnected(report) => self.on_reconnected(report),
            Event::Disconnected { port } => self.on_disconnected(&port),
            Event::TempRead(image) => {
                self.set_image(image);
                self.reconcile_held();
            }
            Event::WriteVerified(outcome) => self.on_verified(job.as_ref(), outcome),
            Event::ChainWritten(outcome) => self.on_chain_written(job.as_ref(), outcome),
            Event::ChannelSwitched(outcome) => {
                self.model.borrow_mut().channel = Some(outcome.channel);
                self.set_clean_image(outcome.temp);
                let text = if outcome.reloaded {
                    trf(
                        "Reloaded {channel} from the amp",
                        &[("channel", outcome.channel.label())],
                    )
                } else if outcome.attempts > 1 {
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
                if let Some(Job::Backup(follow_up)) = job {
                    self.run_follow_up(follow_up);
                } else {
                    tracing::info!(%key, "temp backed up");
                }
            }
            Event::Restored { key, outcome } => {
                self.set_clean_image(outcome.image);
                tracing::info!(%key, "restored to temp");
            }
            Event::PresetSaved { name, .. } => self.on_saved(job.as_ref(), &name),
            Event::PresetUpdated {
                previous,
                key,
                name,
            } => self.on_preset_updated(&previous, key, &name),
            Event::PresetDeleted { key } => self.on_preset_deleted(&key),
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
            Event::Backups(keys) => tracing::debug!(count = keys.len(), "backups listed"),
            Event::Failed(error) => self.on_failed(job, &error),
            Event::FullBackupRead(backup) => {
                if let Some(Job::BackupAmp(path)) = job.clone() {
                    self.write_backup(path, backup);
                }
            }
            Event::LivesetEncoded { text, patches } => self.on_liveset(job.as_ref(), text, patches),
            Event::ShutDown => {}
        }
        if ended {
            self.after_job();
        }
    }

    fn on_verified(&self, job: Option<&Job>, outcome: WriteOutcome) {
        if let Some(Job::SendTone(name)) = job {
            self.set_clean_image(outcome.image);
            self.toast(&trf("Sent “{name}” to temp", &[("name", name)]));
        } else {
            self.on_write_verified(job, outcome);
        }
    }

    fn on_saved(&self, job: Option<&Job>, name: &str) {
        if matches!(job, Some(Job::SaveTone(_))) {
            self.toast(&trf("Saved preset “{name}”", &[("name", name)]));
            self.send(Command::ListPresets, Job::ListPresets);
        } else {
            self.on_preset_saved(name);
        }
    }

    fn on_liveset(self: &Rc<Self>, job: Option<&Job>, text: String, patches: usize) {
        if let Some(Job::ExportPresets(path)) = job {
            self.write_liveset(path.clone(), text, patches);
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
        if error.is_disconnect() {
            tracing::info!(?job, %error, "cancelled, the Katana is gone");
            return;
        }
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
            (Some(Job::Backup(_)), _) => {
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
            (Some(Job::BackupAmp(_)), _) => dialogs::show_error(
                &self.window,
                &tr("Amp Backup Failed"),
                &trf(
                    "No file was written. The amp was only read, nothing was changed: {error}",
                    &[("error", &error.to_string())],
                ),
            ),
            (Some(Job::SendTone(_)), _) => {
                self.toast(&trf(
                    "Could not send the preset to temp: {error}",
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

    fn on_write_verified(&self, job: Option<&Job>, outcome: WriteOutcome) {
        if job == Some(&Job::PutBack) {
            self.model.borrow_mut().lost_edits = None;
        }
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

    fn on_reconnected(self: &Rc<Self>, report: DetectReport) {
        self.toast(&trf(
            "{model} connected",
            &[("model", &report.device.model_name())],
        ));
        self.on_detected(report);
        self.refresh_all();
    }

    fn on_disconnected(self: &Rc<Self>, port: &str) {
        tracing::info!(%port, "Katana disconnected, keeping unsaved edits");
        self.cancel_write_timers();
        let mut writes = std::mem::take(&mut *self.in_flight.borrow_mut());
        writes.extend(std::mem::take(&mut *self.pending.borrow_mut()));
        {
            let mut model = self.model.borrow_mut();
            let profile = model.device.as_ref().and_then(|device| device.profile);
            if let Some(image) = model.image.take() {
                let writes = writes
                    .into_iter()
                    .map(|(id, value)| ParamWrite {
                        id: id.to_owned(),
                        value,
                    })
                    .collect::<Vec<_>>();
                let links = profile.map_or(&[][..], |profile| profile.verify_rules.linked_types);
                let image = apply_param_writes(self.table, &image, &writes, links).unwrap_or(image);
                let baseline = model.baseline.take();
                if model.held.is_none() {
                    model.held = Some(Held { image, baseline });
                }
            }
            model.device = None;
            model.channel = None;
            model.image = None;
            model.baseline = None;
            model.pending_switch = None;
            model.failed_detect = true;
        }
        self.toast(&tr("Katana disconnected"));
        self.end_status();
        self.refresh_all();
    }

    fn reconcile_held(self: &Rc<Self>) {
        let Some(held) = self.model.borrow_mut().held.take() else {
            return;
        };
        let profile = self.profile();
        let mut model = self.model.borrow_mut();
        let Some(current) = model.image.as_ref() else {
            model.held = Some(held);
            return;
        };
        let edited = held
            .baseline
            .as_ref()
            .is_some_and(|baseline| editable_differs(profile, &held.image, baseline));
        if !editable_differs(profile, &held.image, current) {
            tracing::info!(edited, "amp kept the temp across the reconnect");
            if let Some(baseline) = held.baseline {
                model.baseline = Some(baseline);
            }
            return;
        }
        if !edited {
            tracing::info!("amp temp changed while disconnected, nothing unsaved was lost");
            return;
        }
        tracing::info!("amp lost the unsaved edits, offering to put them back");
        model.lost_edits = Some(held.image);
        drop(model);
        let toast = adw::Toast::builder()
            .title(tr("The amp no longer has your unsaved edits"))
            .button_label(tr("Put Back"))
            .action_name("win.put-back-edits")
            .timeout(0)
            .build();
        self.toasts.add_toast(toast);
    }

    fn put_back_edits(&self) {
        if !self.can_write() || self.is_busy() {
            return;
        }
        let image = self.model.borrow().lost_edits.clone();
        if let Some(image) = image {
            self.backup_then(FollowUp::PutBack(image));
        }
    }

    fn on_preset_updated(&self, previous: &str, key: String, name: &str) {
        {
            let mut model = self.model.borrow_mut();
            let image = model.image.clone();
            model.baseline = image;
            if model.chosen_preset.as_deref() == Some(previous) {
                model.chosen_preset = Some(key);
            }
        }
        self.toast(&trf("Updated preset “{name}”", &[("name", name)]));
        self.send(Command::ListPresets, Job::ListPresets);
    }

    fn on_preset_deleted(&self, key: &str) {
        let name = {
            let mut model = self.model.borrow_mut();
            if model.chosen_preset.as_deref() == Some(key) {
                model.chosen_preset = None;
            }
            let name = model
                .presets
                .iter()
                .find(|entry| entry.key == key)
                .map_or_else(|| key.to_owned(), |entry| entry.name.clone());
            model.presets.retain(|entry| entry.key != key);
            name
        };
        self.rebuild_presets();
        self.refresh_sensitivity();
        self.toast(&trf("Deleted preset “{name}”", &[("name", &name)]));
        self.send(Command::ListPresets, Job::ListPresets);
    }

    fn chosen_entry(&self) -> Option<PresetEntry> {
        let model = self.model.borrow();
        let key = model.chosen_preset.as_ref()?;
        model
            .presets
            .iter()
            .find(|entry| &entry.key == key)
            .cloned()
    }

    fn prompt_update_preset(self: &Rc<Self>) {
        if !self.can_write() || self.is_busy() {
            return;
        }
        let Some(entry) = self.chosen_entry() else {
            return;
        };
        let ui = Rc::clone(self);
        let title = entry.name.clone();
        dialogs::confirm_update(&self.window, &title, move |name| {
            ui.flush_writes_now();
            ui.send(
                Command::UpdatePreset {
                    key: entry.key.clone(),
                    name: name.clone(),
                },
                Job::UpdatePreset(name),
            );
        });
    }

    fn prompt_delete_preset(self: &Rc<Self>) {
        if self.is_busy() {
            return;
        }
        let Some(entry) = self.chosen_entry() else {
            return;
        };
        let ui = Rc::clone(self);
        let title = entry.name.clone();
        dialogs::confirm_delete(&self.window, &title, move || {
            ui.send(
                Command::DeletePreset {
                    key: entry.key.clone(),
                },
                Job::DeletePreset(entry.name.clone()),
            );
        });
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

    fn profile(&self) -> Option<&'static DeviceProfile> {
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
        if self.chain_write_queued()
            && let (Some(profile), Some(pattern)) = (self.profile(), self.current_pattern())
            && let Ok(next) = profile.with_chain_pattern(&image, pattern)
        {
            image = next;
        }
        self.set_image(image);
    }

    fn on_chain_written(&self, job: Option<&Job>, outcome: ChainOutcome) {
        let undo = matches!(job, Some(Job::WriteChain { undo: true }));
        let normalised = outcome.is_normalised();
        let changed = outcome.packets > 0;
        let previous = outcome.previous;
        tracing::info!(
            requested = outcome.requested,
            actual = outcome.actual,
            order = %hex(&outcome.order),
            normalised,
            "chain pattern written"
        );
        self.adopt_image(outcome.image);
        if !changed {
            return;
        }
        let title = if normalised {
            tr("The amp kept a different signal chain")
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
                    Job::WriteChain { .. } | Job::WriteParams | Job::ListPresets
                )
            })
    }

    fn write_chain(self: &Rc<Self>, pattern: u8, undo: bool) {
        let Some(profile) = self.profile() else {
            return;
        };
        {
            let mut model = self.model.borrow_mut();
            let Some(image) = model.image.as_mut() else {
                return;
            };
            match profile.with_chain_pattern(image, pattern) {
                Ok(next) => *image = next,
                Err(error) => {
                    drop(model);
                    tracing::warn!(%error, "refusing chain write");
                    self.toast(&error.to_string());
                    return;
                }
            }
        }
        tracing::info!(pattern, "changing the signal chain pattern");
        if let Some(old) = self.chain_toast.take() {
            old.dismiss();
        }
        self.flush_writes_now();
        self.send(Command::WriteChain(pattern), Job::WriteChain { undo });
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
            (Some(image), Some(baseline)) => editable_differs(profile, image, baseline),
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
                let offset = PatchOffset::from_linear(position);
                if image.get(offset).is_some_and(|current| current != value) {
                    changed |= image.set(offset, value).is_ok();
                }
            }
        }
        if changed && !self.is_busy() {
            tracing::info!(%packet, "temp changed on the amp");
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

    fn backup_then(&self, follow_up: FollowUp) {
        self.flush_writes_now();
        let reason = match follow_up {
            FollowUp::ApplyPreset { .. } => "preset",
            FollowUp::PutBack(_) => "put-back",
            FollowUp::SendTone { .. } => "import",
        };
        let key = format!("auto-before-{reason}-{}.syx", Self::timestamp());
        self.send(Command::BackupTemp { key }, Job::Backup(follow_up));
    }

    fn run_follow_up(&self, follow_up: FollowUp) {
        match follow_up {
            FollowUp::ApplyPreset { key, name } => {
                self.send(Command::ApplyPreset { key }, Job::ApplyPreset(name));
            }
            FollowUp::PutBack(image) => self.send(Command::WriteTemp(image), Job::PutBack),
            FollowUp::SendTone { image, name } => {
                self.send(Command::WriteTemp(image), Job::SendTone(name));
            }
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

    fn file_filter(name: &str, suffix: &str) -> (gio::ListStore, gtk::FileFilter) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(name));
        filter.add_suffix(suffix);
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        (filters, filter)
    }

    fn save_dialog(
        self: &Rc<Self>,
        title: &str,
        initial: &str,
        filter: (&str, &str),
        on_path: impl FnOnce(&Rc<Self>, PathBuf) + 'static,
    ) {
        let (filters, default) = Self::file_filter(filter.0, filter.1);
        let dialog = gtk::FileDialog::builder()
            .title(title)
            .modal(true)
            .initial_name(initial)
            .filters(&filters)
            .default_filter(&default)
            .build();
        let ui = Rc::clone(self);
        dialog.save(
            Some(&self.window),
            gio::Cancellable::NONE,
            move |result| match result.map(|file| file.path()) {
                Ok(Some(path)) => on_path(&ui, path),
                Ok(None) => ui.toast(&tr("Choose a local file")),
                Err(error) => tracing::debug!(%error, "save dialog closed"),
            },
        );
    }

    fn run_blocking<T: Send + 'static>(
        self: &Rc<Self>,
        work: impl FnOnce() -> Result<T> + Send + 'static,
        done: impl FnOnce(&Rc<Self>, Result<T>) + 'static,
    ) {
        let ui = Rc::clone(self);
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(work)
                .await
                .map_err(|_| anyhow::anyhow!("the file worker stopped unexpectedly"))
                .and_then(|result| result);
            done(&ui, result);
        });
    }

    fn choose_backup_file(self: &Rc<Self>) {
        if !self.can_write() || self.is_busy() {
            return;
        }
        let stamp = glib::DateTime::now_local()
            .and_then(|now| now.format("%Y%m%d-%H%M"))
            .map_or_else(|_| "now".to_owned(), |text| text.to_string());
        self.save_dialog(
            &tr("Back Up Amp"),
            &format!("katana-backup-{stamp}.alb"),
            (&tr("BOSS TONE STUDIO backup (.alb)"), "alb"),
            |ui, path| {
                if !ui.can_write() || ui.is_busy() {
                    ui.toast(&tr("The amp is busy, try again"));
                    return;
                }
                ui.flush_writes_now();
                ui.send(Command::ReadFullBackup, Job::BackupAmp(path));
            },
        );
    }

    fn write_backup(self: &Rc<Self>, path: PathBuf, backup: FullBackup) {
        let target = path.clone();
        self.run_blocking(
            move || files::save_backup(&target, &backup),
            move |ui, result| match result {
                Ok(rows) => ui.toast(&trf(
                    "Amp backed up to {file} ({rows} stored patches)",
                    &[("file", &display_name(&path)), ("rows", &rows.to_string())],
                )),
                Err(error) => dialogs::show_error(
                    &ui.window,
                    &tr("Amp Backup Failed"),
                    &trf(
                        "The amp was read but the file could not be written: {error}",
                        &[("error", &format!("{error:#}"))],
                    ),
                ),
            },
        );
    }

    fn choose_tone_file(self: &Rc<Self>) {
        let (filters, default) = Self::file_filter(&tr("BOSS TONE STUDIO liveset (.tsl)"), "tsl");
        let dialog = gtk::FileDialog::builder()
            .title(tr("Import Preset"))
            .modal(true)
            .filters(&filters)
            .default_filter(&default)
            .build();
        let ui = Rc::clone(self);
        dialog.open(
            Some(&self.window),
            gio::Cancellable::NONE,
            move |result| match result.map(|file| file.path()) {
                Ok(Some(path)) => ui.load_tone_file(path),
                Ok(None) => ui.toast(&tr("Choose a local file")),
                Err(error) => tracing::debug!(%error, "open dialog closed"),
            },
        );
    }

    fn load_tone_file(self: &Rc<Self>, path: PathBuf) {
        let source = path.clone();
        self.run_blocking(
            move || files::load_tones(&source),
            move |ui, result| match result {
                Ok(file) if file.tones.is_empty() => ui.toast(&tr("The file holds no patches")),
                Ok(file) => ui.prompt_tone(file),
                Err(error) => dialogs::show_error(
                    &ui.window,
                    &tr("Cannot Import Preset"),
                    &trf(
                        "{file}: {error}",
                        &[
                            ("file", &display_name(&path)),
                            ("error", &format!("{error:#}")),
                        ],
                    ),
                ),
            },
        );
    }

    fn prompt_tone(self: &Rc<Self>, file: ToneFile) {
        let tones: Vec<Option<(PatchImage, String)>> = file
            .tones
            .iter()
            .map(|tone| {
                tone.loaded.as_ref().ok().map(|(image, _)| {
                    let name = image.name().trim().to_owned();
                    (image.clone(), name)
                })
            })
            .collect();
        let ui = Rc::clone(self);
        dialogs::choose_tone(
            &self.window,
            file,
            self.can_write(),
            move |index, target| {
                let Some(Some((image, name))) = tones.get(index).cloned() else {
                    return;
                };
                match target {
                    ToneTarget::Preset => ui.prompt_tone_name(image, &name),
                    ToneTarget::Temp => {
                        if !ui.can_write() || ui.is_busy() {
                            ui.toast(&tr("The amp is busy, try again"));
                            return;
                        }
                        let label = if name.is_empty() {
                            tr("Imported preset")
                        } else {
                            name
                        };
                        ui.backup_then(FollowUp::SendTone { image, name: label });
                    }
                }
            },
        );
    }

    fn prompt_tone_name(self: &Rc<Self>, image: PatchImage, suggestion: &str) {
        let ui = Rc::clone(self);
        dialogs::ask_tone_name(&self.window, suggestion, move |name| {
            let exists = ui
                .model
                .borrow()
                .presets
                .iter()
                .any(|entry| entry.name.eq_ignore_ascii_case(&name));
            let save_ui = Rc::clone(&ui);
            let save_name = name.clone();
            let save_image = image.clone();
            let save = move || {
                save_ui.send(
                    Command::SaveImportedPreset {
                        name: save_name.clone(),
                        image: save_image.clone(),
                    },
                    Job::SaveTone(save_name.clone()),
                );
            };
            if exists {
                dialogs::confirm_replace(&ui.window, &name, save);
            } else {
                save();
            }
        });
    }

    fn prompt_export(self: &Rc<Self>) {
        let presets = self.model.borrow().presets.clone();
        if presets.is_empty() {
            self.toast(&tr("No presets yet"));
            return;
        }
        let ui = Rc::clone(self);
        dialogs::choose_export(
            &self.window,
            &presets,
            TSL_MAX_PATCHES,
            move |keys, name| {
                let initial = format!("{}.tsl", name.replace(['/', '\\'], "-"));
                ui.save_dialog(
                    &tr("Export Presets"),
                    &initial,
                    (&tr("BOSS TONE STUDIO liveset (.tsl)"), "tsl"),
                    move |ui, path| {
                        ui.send(
                            Command::ExportLiveset { keys, name },
                            Job::ExportPresets(path),
                        );
                    },
                );
            },
        );
    }

    fn write_liveset(self: &Rc<Self>, path: PathBuf, text: String, patches: usize) {
        let target = path.clone();
        self.run_blocking(
            move || files::write_atomic(&target, text.as_bytes()),
            move |ui, result| match result {
                Ok(()) => ui.toast(&trf(
                    "Exported {count} preset(s) to {file}",
                    &[
                        ("count", &patches.to_string()),
                        ("file", &display_name(&path)),
                    ],
                )),
                Err(error) => {
                    dialogs::show_error(&ui.window, &tr("Export Failed"), &format!("{error:#}"));
                }
            },
        );
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
                    "AmpGhost currently supports only the MK II series.\nDetected identity: {identity}\nAll writes are disabled.",
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
            None => tr("AmpGhost"),
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
            button.set_visible(profile.unwrap_or(&KATANA_50_MK2).has_channel(*channel));
            button.set_tooltip_text(Some(&trf(
                "Switch to {channel}",
                &[("channel", channel.label())],
            )));
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
            let available = profile.unwrap_or(&KATANA_50_MK2).has_channel(*channel);
            button.set_sensitive(writable && available && profile.is_some() && !busy);
        }
        self.preset_list.set_sensitive(writable && !busy);
        let chosen = self.chosen_entry().is_some();
        for (name, enabled) in [
            ("update-preset", writable && !busy && chosen),
            ("delete-preset", !busy && chosen),
        ] {
            if let Some(action) = self
                .window
                .lookup_action(name)
                .and_then(|action| action.downcast::<gio::SimpleAction>().ok())
            {
                action.set_enabled(enabled);
            }
        }
        let has_presets = !self.model.borrow().presets.is_empty();
        for (name, enabled) in [
            ("backup-amp", writable && !busy),
            ("import-tone", !busy),
            ("export-presets", !busy && has_presets),
        ] {
            if let Some(action) = self
                .window
                .lookup_action(name)
                .and_then(|action| action.downcast::<gio::SimpleAction>().ok())
            {
                action.set_enabled(enabled);
            }
        }
        for name in ["save-preset", "reload"] {
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
        self.refresh_chain_picker();
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

    fn refresh_chain_picker(&self) {
        let previous = self.updating.replace(true);
        let selected = self
            .current_pattern()
            .map_or(gtk::INVALID_LIST_POSITION, u32::from);
        if self.chain_picker.selected() != selected {
            self.chain_picker.set_selected(selected);
        }
        self.updating.set(previous);
        self.chain_picker
            .set_sensitive(self.chain_editable() && self.current_pattern().is_some());
    }

    fn rebuild_presets(&self) {
        self.preset_list.remove_all();
        let model = self.model.borrow();
        let mut chosen_row = None;
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
            row.add_suffix(
                &gtk::Image::builder()
                    .icon_name("ampghost-upload-symbolic")
                    .valign(gtk::Align::Center)
                    .css_classes(["preset-apply"])
                    .accessible_role(gtk::AccessibleRole::Presentation)
                    .build(),
            );
            self.preset_list.append(&row);
            if model.chosen_preset.as_ref() == Some(&entry.key) {
                chosen_row = Some(row.clone());
            }
        }
        drop(model);
        if let Some(row) = chosen_row {
            self.preset_list.select_row(Some(&row));
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
                let text = trf(
                    "Position {position} of {count} in the signal chain. Choose the order with Signal Chain Order.",
                    &[("position", &position), ("count", &count)],
                );
                card.root
                    .update_property(&[gtk::accessible::Property::Description(&text)]);
            }
            refresh_node_state(card, card.key == model.selected);
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
        self.refresh_chain_picker();
        self.fit_detail();
    }

    fn rebuild_chain(self: &Rc<Self>, order: &[&'static BlockSpec]) {
        for container in [&self.chain_box, &self.extras_box] {
            while let Some(child) = container.first_child() {
                container.remove(&child);
            }
        }
        let mut cards = Vec::new();
        for spec in order {
            if spec.mini {
                self.chain_box.append(&mini_node(spec));
                continue;
            }
            let card = self.make_card(spec);
            if spec.chain_label.is_none() {
                self.extras_box.append(&card.root);
            } else {
                self.chain_box.append(&card.root);
            }
            cards.push(card);
        }
        *self.cards.borrow_mut() = cards;
        self.fit_detail();
    }

    fn fit_detail(&self) {
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
        Card {
            key: spec.key,
            root,
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

fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn editable_differs(
    profile: Option<&'static DeviceProfile>,
    image: &PatchImage,
    other: &PatchImage,
) -> bool {
    image
        .differing_offsets(other)
        .iter()
        .any(|offset| profile.is_none_or(|profile| !profile.verify_rules.is_amp_derived(*offset)))
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
        .active(false)
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
        .show_sidebar(false)
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
            .label(tr("AmpGhost"))
            .css_classes(["heading"])
            .build(),
    );
    header.set_title_widget(Some(&adw::Bin::new()));
    (header, toggle)
}

fn build_header() -> Header {
    let title = adw::WindowTitle::new(&tr("AmpGhost"), "");
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
        .halign(gtk::Align::Start)
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
        .selection_mode(gtk::SelectionMode::Single)
        .build();
    preset_list.set_placeholder(Some(
        &gtk::Label::builder()
            .label(tr("No presets yet"))
            .css_classes(["dim-label"])
            .margin_top(24)
            .build(),
    ));
    let save_button = gtk::Button::builder()
        .label(tr("Save"))
        .tooltip_text(tr("Save Current Sound as Preset (Ctrl+S)"))
        .action_name("win.save-preset")
        .css_classes(["suggested-action"])
        .hexpand(true)
        .build();
    let update_button = gtk::Button::builder()
        .label(tr("Update"))
        .tooltip_text(tr("Update the Selected Preset with the Current Sound"))
        .action_name("win.update-preset")
        .hexpand(true)
        .build();
    let delete_button = gtk::Button::builder()
        .label(tr("Delete"))
        .tooltip_text(tr("Delete the Selected Preset"))
        .action_name("win.delete-preset")
        .hexpand(true)
        .build();
    let preset_buttons = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .homogeneous(true)
        .css_classes(["linked"])
        .build();
    preset_buttons.append(&save_button);
    preset_buttons.append(&update_button);
    preset_buttons.append(&delete_button);
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
    sidebar.append(&preset_buttons);

    (sidebar, preset_search, preset_list)
}
