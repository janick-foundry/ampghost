use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::sync::mpsc;
use std::time::Duration;

use adw::prelude::*;
use anyhow::{Context, Result};
use gtk::{gio, glib};
use katana_engine::{
    Command, DetectReport, DeviceInfo, EngineError, EngineHandle, Event, ParamWrite, PresetEntry,
    Stage,
};
use katana_params::ParamTable;
use katana_protocol::{Channel, Dt1, PatchImage, PatchOffset, parse_current_channel};

use crate::blocks::{self, BlockSpec, Variant};
use crate::bridge;
use crate::dialogs::{self, SwitchChoice};
use crate::i18n::{tr, trf};
use crate::params_view::{self, OnChange, ParamsView, Structure};

const CHANNEL_ROW_WIDTH: i32 = 600;
const CHANNEL_ROW_TIGHTENING: i32 = 400;
const EVENT_PUMP: Duration = Duration::from_millis(30);
const WRITE_DEBOUNCE: Duration = Duration::from_millis(250);
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
    presets: Vec<PresetEntry>,
    selected: &'static str,
    pending_switch: Option<Channel>,
    failed_detect: bool,
}

struct Card {
    key: &'static str,
    root: gtk::Box,
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
    params_holder: adw::Bin,
    preset_list: gtk::ListBox,
    preset_search: gtk::SearchEntry,
    sidebar: gtk::Box,
    engine: RefCell<Option<EngineHandle>>,
    model: RefCell<Model>,
    jobs: RefCell<VecDeque<Job>>,
    updating: Rc<Cell<bool>>,
    pending: RefCell<BTreeMap<&'static str, u16>>,
    debounce: Cell<u64>,
    status_serial: Cell<u64>,
    closing: Cell<bool>,
    cards: RefCell<Vec<Card>>,
    chain_keys: RefCell<Vec<&'static str>>,
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

impl Ui {
    fn build(app: &adw::Application, table: &'static ParamTable, engine: EngineHandle) -> Rc<Self> {
        let (header, title, status_box, status_label, sidebar_button) = build_header();
        let (channel_section, channel_buttons) = build_channels();
        let (sidebar, preset_search, preset_list) = build_sidebar();
        let chain_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(6)
            .css_classes(["chain-row"])
            .build();
        let chain_scroller = gtk::ScrolledWindow::builder()
            .vscrollbar_policy(gtk::PolicyType::Never)
            .child(&chain_box)
            .build();
        let params_holder = adw::Bin::builder().vexpand(true).build();
        let editor = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .build();
        editor.append(&channel_section);
        editor.append(
            &gtk::Label::builder()
                .label(tr("Signal Chain"))
                .xalign(0.0)
                .margin_start(16)
                .margin_top(12)
                .css_classes(["heading"])
                .build(),
        );
        editor.append(&chain_scroller);
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
            .default_width(1280)
            .default_height(820)
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
            debounce: Cell::new(0),
            status_serial: Cell::new(0),
            closing: Cell::new(false),
            cards: RefCell::new(Vec::new()),
            chain_keys: RefCell::new(Vec::new()),
            params: RefCell::new(None),
            search_text: Rc::new(RefCell::new(String::new())),
        });
        ui.connect_sidebar();
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

    fn on_close_request(&self) -> glib::Propagation {
        if !self.pending.borrow().is_empty() || self.is_busy() {
            self.closing.set(true);
            self.flush_writes();
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
                self.set_image(outcome.image);
            }
            Event::ChannelSwitched(outcome) => {
                self.model.borrow_mut().channel = Some(outcome.channel);
                self.set_image(outcome.temp);
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
                self.set_image(outcome.image);
                self.toast(&trf("Restored {key} to temp", &[("key", &key)]));
            }
            Event::PresetSaved { name, .. } => {
                self.toast(&trf("Saved preset “{name}”", &[("name", &name)]));
                self.send(Command::ListPresets, Job::ListPresets);
                let next = self.model.borrow_mut().pending_switch.take();
                if let Some(channel) = next {
                    self.switch_now(channel);
                }
            }
            Event::PresetApplied { name, outcome, .. } => {
                self.set_image(outcome.image);
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
            model.failed_detect = false;
        }
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
                self.pending.borrow_mut().clear();
                self.toast(&trf(
                    "Could not write the change: {error}",
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

    fn set_image(&self, image: PatchImage) {
        self.model.borrow_mut().image = Some(image);
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
                if !self.is_busy() {
                    self.send(Command::ReadTemp, Job::ReadTemp);
                }
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

    fn queue_write(self: &Rc<Self>, id: &'static str, value: u16) {
        if !self.can_write() {
            return;
        }
        self.pending.borrow_mut().insert(id, value);
        let generation = self.debounce.get().wrapping_add(1);
        self.debounce.set(generation);
        let ui = Rc::downgrade(self);
        glib::timeout_add_local_once(WRITE_DEBOUNCE, move || {
            if let Some(ui) = ui.upgrade()
                && ui.debounce.get() == generation
            {
                ui.flush_writes();
            }
        });
    }

    fn flush_writes(&self) {
        if self.is_busy() || !self.can_write() {
            return;
        }
        let writes: Vec<ParamWrite> = std::mem::take(&mut *self.pending.borrow_mut())
            .into_iter()
            .map(|(id, value)| ParamWrite {
                id: id.to_owned(),
                value,
            })
            .collect();
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
        let key = format!("temp-{}.syx", Self::timestamp());
        self.send(Command::BackupTemp { key }, Job::Backup { then: None });
    }

    fn backup_then(&self, follow_up: FollowUp) {
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
        let ui = Rc::clone(self);
        dialogs::confirm_switch(&self.window, channel, move |choice| match choice {
            SwitchChoice::Switch => ui.switch_now(channel),
            SwitchChoice::SaveFirst => ui.prompt_save_preset(Some(channel)),
        });
    }

    fn switch_now(&self, channel: Channel) {
        self.pending.borrow_mut().clear();
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
        let model = self.model.borrow();
        let patch = model
            .image
            .as_ref()
            .map(PatchImage::name)
            .filter(|name| !name.is_empty());
        self.title
            .set_title(&patch.unwrap_or_else(|| tr("Katana Control")));
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
        let previous = self.updating.replace(true);
        for card in self.cards.borrow().iter() {
            let Some(spec) = blocks::spec(card.key) else {
                continue;
            };
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
        while let Some(child) = self.chain_box.first_child() {
            self.chain_box.remove(&child);
        }
        let mut cards = Vec::new();
        let chained = order
            .iter()
            .filter(|spec| spec.chain_label.is_some())
            .count();
        for (index, spec) in order.iter().enumerate() {
            if index > 0 {
                let separator = if index == chained {
                    gtk::Separator::new(gtk::Orientation::Vertical).upcast::<gtk::Widget>()
                } else {
                    gtk::Image::builder()
                        .icon_name("go-next-symbolic")
                        .css_classes(["chain-arrow"])
                        .build()
                        .upcast::<gtk::Widget>()
                };
                separator.set_margin_start(2);
                separator.set_margin_end(2);
                self.chain_box.append(&separator);
            }
            let card = self.make_card(spec);
            self.chain_box.append(&card.root);
            cards.push(card);
        }
        *self.cards.borrow_mut() = cards;
    }

    fn make_card(self: &Rc<Self>, spec: &'static BlockSpec) -> Card {
        let root = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .css_classes(["card", "chain-block"])
            .build();
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
        root.append(&button);
        let switch = spec.on_param.map(|id| {
            let switch = gtk::Switch::builder().halign(gtk::Align::Center).build();
            switch.update_property(&[gtk::accessible::Property::Label(&trf(
                "Enable {block}",
                &[("block", &tr(spec.title))],
            ))]);
            let write_ui = Rc::clone(self);
            switch.connect_active_notify(move |switch| {
                if !write_ui.updating.get() {
                    write_ui.queue_write(id, u16::from(switch.is_active()));
                }
            });
            root.append(&switch);
            switch
        });
        Card {
            key: spec.key,
            root,
            dot,
            type_label,
            switch,
        }
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
        let on_change: OnChange = Rc::new(move |id, value| {
            if let Some(ui) = weak.upgrade() {
                ui.queue_write(id, value);
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
        .maximum_size(CHANNEL_ROW_WIDTH)
        .tightening_threshold(CHANNEL_ROW_TIGHTENING)
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
