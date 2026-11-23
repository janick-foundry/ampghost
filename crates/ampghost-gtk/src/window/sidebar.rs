use std::rc::Rc;

use adw::prelude::*;
use ampghost_engine::{Command, PresetEntry};
use ampghost_protocol::{Channel, PatchImage};

use crate::dialogs;
use crate::i18n::{tr, trf};
use crate::jobs::{FollowUp, Job};

use super::Ui;

pub(super) fn sidebar_toggle() -> gtk::ToggleButton {
    gtk::ToggleButton::builder()
        .icon_name("sidebar-show-symbolic")
        .tooltip_text(tr("Show Presets"))
        .active(false)
        .build()
}

pub(super) fn build_split(
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

pub(super) fn build_sidebar_header() -> (adw::HeaderBar, gtk::ToggleButton) {
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

pub(super) fn build_sidebar() -> (gtk::Box, gtk::SearchEntry, gtk::ListBox) {
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

impl Ui {
    pub(super) fn connect_sidebar(self: &Rc<Self>) {
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

    pub(super) fn on_preset_updated(&self, previous: &str, key: String, name: &str) {
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

    pub(super) fn on_preset_deleted(&self, key: &str) {
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

    pub(super) fn chosen_entry(&self) -> Option<PresetEntry> {
        let model = self.model.borrow();
        let key = model.chosen_preset.as_ref()?;
        model
            .presets
            .iter()
            .find(|entry| &entry.key == key)
            .cloned()
    }

    pub(super) fn prompt_update_preset(self: &Rc<Self>) {
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

    pub(super) fn prompt_delete_preset(self: &Rc<Self>) {
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

    pub(super) fn on_preset_saved(&self, name: &str) {
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

    pub(super) fn apply_preset(&self, entry: PresetEntry) {
        if !self.can_write() || self.is_busy() {
            return;
        }
        self.backup_then(FollowUp::ApplyPreset {
            key: entry.key,
            name: entry.name,
        });
    }

    pub(super) fn prompt_save_preset(self: &Rc<Self>, then_switch: Option<Channel>) {
        if !self.can_write() {
            return;
        }
        let suggestion = self
            .model
            .borrow()
            .image
            .as_ref()
            .map_or_else(String::new, PatchImage::name);
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

    pub(super) fn rebuild_presets(&self) {
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
}
