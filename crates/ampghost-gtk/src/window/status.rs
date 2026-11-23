use std::rc::Rc;
use std::time::Duration;

use adw::prelude::*;
use ampghost_engine::{Command, Stage};
use ampghost_protocol::{Channel, KATANA_50_MK2, PatchImage};
use gtk::glib;

use crate::dialogs::{self, SwitchChoice};
use crate::i18n::{tr, trf};
use crate::jobs::Job;
use crate::params_view;

use super::chain::TOGGLE_FADE;
use super::sidebar::sidebar_toggle;
use super::{Ui, main_menu};

const STATUS_DELAY: Duration = Duration::from_millis(300);

pub(super) type Header = (
    adw::HeaderBar,
    adw::WindowTitle,
    gtk::Label,
    gtk::Box,
    gtk::Revealer,
);

pub(super) fn build_header() -> Header {
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

pub(super) fn build_channels() -> (adw::Clamp, Vec<(Channel, gtk::Button)>) {
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

impl Ui {
    pub(super) fn begin_status(&self, job: &Job) {
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

    pub(super) fn show_status_now(&self, text: &str) {
        self.status_active.set(true);
        self.status_label.set_text(text);
    }

    pub(super) fn end_status(&self) {
        self.status_serial
            .set(self.status_serial.get().wrapping_add(1));
        self.status_active.set(false);
        self.refresh_light();
    }

    pub(super) fn show_stage(&self, stage: Stage) {
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

    pub(super) fn request_switch(self: &Rc<Self>, channel: Channel) {
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

    pub(super) fn switch_now(&self, channel: Channel) {
        self.drop_pending_writes("switching channel");
        self.send(Command::SwitchChannel(channel), Job::Switch(channel));
    }

    pub(super) fn refresh_stack(&self) {
        let model = self.model.borrow();
        let searching = self.jobs.borrow().contains(&Job::Detect);
        let page = &self.status_page;
        page.set_paintable(None::<&gtk::gdk::Paintable>);
        self.retry_button.set_visible(false);
        match (&model.device, &model.image) {
            (Some(device), _) if !device.is_supported() => {
                page.set_icon_name(Some("ampghost-warning-symbolic"));
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
                page.set_icon_name(Some("ampghost-speakers-symbolic"));
                page.set_title(&tr("Katana Not Connected"));
                page.set_description(Some(&tr("Connect the amp via USB and turn it on.")));
                self.retry_button.set_visible(true);
            }
        }
        self.stack.set_visible_child_name("status");
    }

    pub(super) fn refresh_header(&self) {
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

    pub(super) fn refresh_light(&self) {
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
}
