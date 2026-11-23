use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::rc::Rc;

use adw::prelude::*;
use ampghost_engine::{
    AmpSwitch, Command, DetectReport, EngineError, EngineHandle, Envelope, Event, ParamWrite,
    WriteOutcome, apply_param_writes,
};
use ampghost_params::ParamTable;
use ampghost_protocol::{
    Channel, DeviceProfile, Dt1, KATANA_50_MK2, PatchImage, PatchOffset, parse_current_channel,
};
use anyhow::{Context, Result};
use futures_channel::mpsc::UnboundedReceiver;
use futures_util::StreamExt;
use gtk::{gio, glib};

use crate::bridge;
use crate::dialogs;
use crate::i18n::{tr, trf};
use crate::jobs::{FollowUp, Job, Jobs};
use crate::params_view::ParamsView;
use crate::report;

mod chain;
mod detail;
mod model;
mod sidebar;
mod status;
mod transfer;

use chain::{Card, build_blocks_row, chain_picker};
use detail::{build_detail, build_editor};
use model::{Held, Model, editable_differs, writable};
use sidebar::{build_sidebar, build_split};
use status::{build_channels, build_header};

type Handler = Box<dyn Fn(&Rc<Ui>)>;

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
    jobs: RefCell<Jobs>,
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

fn pump_events(ui: &Rc<Ui>, mut events: UnboundedReceiver<Envelope>) {
    let ui = Rc::clone(ui);
    glib::spawn_future_local(async move {
        while let Some(envelope) = events.next().await {
            ui.handle_event(envelope);
        }
        if ui.engine.borrow().is_some() {
            tracing::warn!("engine event channel closed unexpectedly");
        } else {
            tracing::debug!("engine event channel closed");
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

const WINDOW_MIN_WIDTH: i32 = 960;

const WINDOW_MIN_HEIGHT: i32 = 600;

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
        if ampghost_engine::DEV_BUILD {
            window.add_css_class("devel");
        }

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
            jobs: RefCell::new(Jobs::default()),
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
            Ok(request) => {
                tracing::debug!(%request, ?job, "queued");
                let starts = self.jobs.borrow().is_empty();
                if starts {
                    self.begin_status(&job);
                }
                self.jobs.borrow_mut().push(request, job);
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

    fn handle_event(self: &Rc<Self>, envelope: Envelope) {
        let Envelope { request, event } = envelope;
        let job = match request {
            Some(request) if event.is_terminal() => {
                let job = self.jobs.borrow_mut().finish(request);
                if job.is_none() {
                    tracing::debug!(%request, "answer to a request without a job");
                }
                job
            }
            _ => None,
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
            tracing::info!(
                ?job,
                error = error as &(dyn std::error::Error + 'static),
                "cancelled, the Katana is gone"
            );
            return;
        }
        tracing::warn!(
            ?job,
            error = error as &(dyn std::error::Error + 'static),
            "command failed"
        );
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
                    &[("error", &report::chain(error))],
                ));
            }
            (Some(Job::WriteParams), _) => {
                self.drop_pending_writes("write failed");
                self.toast(&trf(
                    "Could not write the change: {error}",
                    &[("error", &report::chain(error))],
                ));
                self.send(Command::ReadTemp, Job::ReadTemp);
            }
            (Some(Job::WriteChain { .. }), _) => {
                self.toast(&trf(
                    "Could not change the signal chain: {error}",
                    &[("error", &report::chain(error))],
                ));
                self.send(Command::ReadTemp, Job::ReadTemp);
            }
            (Some(Job::BackupAmp(_)), _) => dialogs::show_error(
                &self.window,
                &tr("Amp Backup Failed"),
                &trf(
                    "No file was written. The amp was only read, nothing was changed: {error}",
                    &[("error", &report::chain(error))],
                ),
            ),
            (Some(Job::SendTone(_)), _) => {
                self.toast(&trf(
                    "Could not send the preset to temp: {error}",
                    &[("error", &report::chain(error))],
                ));
                self.send(Command::ReadTemp, Job::ReadTemp);
            }
            (Some(Job::SavePreset), _) => {
                self.model.borrow_mut().pending_switch = None;
                self.toast(&report::chain(error));
            }
            _ => self.toast(&report::chain(error)),
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

    fn refresh_all(self: &Rc<Self>) {
        self.refresh_stack();
        self.refresh_header();
        self.refresh_chain();
        self.refresh_params();
        self.refresh_sensitivity();
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
}
