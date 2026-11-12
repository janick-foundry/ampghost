use adw::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;

use tsuba_engine::{APP_ID, PresetEntry};
use tsuba_params::SOURCE_ATTRIBUTION;
use tsuba_protocol::Channel;

use crate::files::ToneFile;
use crate::i18n::{tr, trf};

const LUCIDE_LICENSE: &str = include_str!("../data/icons/LICENSE-lucide");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwitchChoice {
    Switch,
    SaveFirst,
}

pub fn confirm_switch(
    parent: &impl IsA<gtk::Widget>,
    channel: Channel,
    on_choice: impl Fn(SwitchChoice) + 'static,
) {
    let label = channel.label();
    let dialog = adw::AlertDialog::new(
        Some(&trf("Switch to {channel}?", &[("channel", label)])),
        Some(&trf(
            "The amp loads the sound stored in {channel}. Unsaved changes to the current sound will be lost.",
            &[("channel", label)],
        )),
    );
    dialog.add_responses(&[
        ("cancel", &tr("Cancel")),
        ("save", &tr("Save as Preset First…")),
        ("switch", &tr("Switch")),
    ]);
    dialog.set_response_appearance("switch", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(None, move |_, response| match response {
        "switch" => on_choice(SwitchChoice::Switch),
        "save" => on_choice(SwitchChoice::SaveFirst),
        _ => {}
    });
    dialog.present(Some(parent));
}

pub fn ask_preset_name(
    parent: &impl IsA<gtk::Widget>,
    suggestion: &str,
    on_name: impl Fn(String) + 'static,
) {
    let dialog = adw::AlertDialog::new(
        Some(&tr("Save Current Sound as Preset")),
        Some(&tr(
            "The preset is stored on this computer. Amp channels are not changed.",
        )),
    );
    let entry = gtk::Entry::builder()
        .text(suggestion)
        .activates_default(true)
        .placeholder_text(tr("Preset Name"))
        .build();
    entry.update_property(&[gtk::accessible::Property::Label(&tr("Preset Name"))]);
    dialog.set_extra_child(Some(&entry));
    dialog.add_responses(&[("cancel", &tr("Cancel")), ("save", &tr("Save"))]);
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");
    let handler_entry = entry.clone();
    dialog.connect_response(None, move |_, response| {
        let name = handler_entry.text().trim().to_owned();
        if response == "save" && !name.is_empty() {
            on_name(name);
        }
    });
    let sync_dialog = dialog.clone();
    entry.connect_changed(move |entry| {
        sync_dialog.set_response_enabled("save", !entry.text().trim().is_empty());
    });
    dialog.present(Some(parent));
    entry.grab_focus();
}

pub fn confirm_replace(parent: &impl IsA<gtk::Widget>, name: &str, on_yes: impl Fn() + 'static) {
    let dialog = adw::AlertDialog::new(
        Some(&trf("Replace “{name}”?", &[("name", name)])),
        Some(&tr("A preset with this name already exists.")),
    );
    dialog.add_responses(&[("cancel", &tr("Cancel")), ("replace", &tr("Replace"))]);
    dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(None, move |_, response| {
        if response == "replace" {
            on_yes();
        }
    });
    dialog.present(Some(parent));
}

pub fn confirm_update(
    parent: &impl IsA<gtk::Widget>,
    name: &str,
    on_name: impl Fn(String) + 'static,
) {
    let dialog = adw::AlertDialog::new(
        Some(&trf("Update “{name}”?", &[("name", name)])),
        Some(&tr(
            "The preset is replaced with the current sound read from the amp. You can rename it at the same time. Amp channels are not changed.",
        )),
    );
    let entry = gtk::Entry::builder()
        .text(name)
        .activates_default(true)
        .placeholder_text(tr("Preset Name"))
        .build();
    entry.update_property(&[gtk::accessible::Property::Label(&tr("Preset Name"))]);
    dialog.set_extra_child(Some(&entry));
    dialog.add_responses(&[("cancel", &tr("Cancel")), ("update", &tr("Update"))]);
    dialog.set_response_appearance("update", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("update"));
    dialog.set_close_response("cancel");
    let handler_entry = entry.clone();
    dialog.connect_response(None, move |_, response| {
        let name = handler_entry.text().trim().to_owned();
        if response == "update" && !name.is_empty() {
            on_name(name);
        }
    });
    let sync_dialog = dialog.clone();
    entry.connect_changed(move |entry| {
        sync_dialog.set_response_enabled("update", !entry.text().trim().is_empty());
    });
    dialog.present(Some(parent));
    entry.grab_focus();
}

pub fn confirm_delete(parent: &impl IsA<gtk::Widget>, name: &str, on_yes: impl Fn() + 'static) {
    let dialog = adw::AlertDialog::new(
        Some(&trf("Delete “{name}”?", &[("name", name)])),
        Some(&tr(
            "The preset file is removed from this computer. Amp channels are not changed.",
        )),
    );
    dialog.add_responses(&[("cancel", &tr("Cancel")), ("delete", &tr("Delete"))]);
    dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(None, move |_, response| {
        if response == "delete" {
            on_yes();
        }
    });
    dialog.present(Some(parent));
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToneTarget {
    Preset,
    Temp,
}

pub fn show_error(parent: &impl IsA<gtk::Widget>, title: &str, body: &str) {
    let dialog = adw::AlertDialog::new(Some(title), Some(body));
    dialog.add_responses(&[("close", &tr("Close"))]);
    dialog.set_default_response(Some("close"));
    dialog.set_close_response("close");
    dialog.present(Some(parent));
}

pub fn choose_tone(
    parent: &impl IsA<gtk::Widget>,
    file: ToneFile,
    can_send: bool,
    on_choice: impl Fn(usize, ToneTarget) + 'static,
) {
    let count = file.tones.len().to_string();
    let dialog = adw::AlertDialog::new(
        Some(&tr("Import Preset")),
        Some(&trf(
            "Liveset “{name}” holds {count} patch(es). Saving as a preset stays on this computer. Sending to temp replaces the current sound in the amp’s temporary patch after backing it up. Amp channels are not changed.",
            &[("name", &file.name), ("count", &count)],
        )),
    );
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let labels: Vec<&str> = file.tones.iter().map(|tone| tone.label.as_str()).collect();
    let dropdown = gtk::DropDown::from_strings(&labels);
    dropdown.update_property(&[gtk::accessible::Property::Label(&tr("Patch"))]);
    dropdown.set_visible(file.tones.len() > 1);
    let report = gtk::Label::builder()
        .wrap(true)
        .xalign(0.0)
        .selectable(true)
        .css_classes(["dim-label", "caption"])
        .build();
    content.append(&dropdown);
    content.append(&report);
    dialog.set_extra_child(Some(&content));
    dialog.add_responses(&[
        ("cancel", &tr("Cancel")),
        ("preset", &tr("Save as Local Preset")),
        ("temp", &tr("Send to Temp")),
    ]);
    dialog.set_response_appearance("preset", adw::ResponseAppearance::Suggested);
    dialog.set_response_appearance("temp", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    let file = Rc::new(file);
    let sync = {
        let file = Rc::clone(&file);
        let dialog = dialog.clone();
        let report = report.clone();
        move |selected: u32| {
            let tone = usize::try_from(selected)
                .ok()
                .and_then(|index| file.tones.get(index));
            let (text, valid) = match tone.map(|tone| &tone.loaded) {
                Some(Ok((_, text))) => (text.clone(), true),
                Some(Err(error)) => (
                    trf(
                        "This patch cannot be imported: {error}",
                        &[("error", error)],
                    ),
                    false,
                ),
                None => (String::new(), false),
            };
            report.set_text(&text);
            dialog.set_response_enabled("preset", valid);
            dialog.set_response_enabled("temp", valid && can_send);
        }
    };
    sync(dropdown.selected());
    dropdown.connect_selected_notify(move |dropdown| sync(dropdown.selected()));
    dialog.connect_response(None, move |_, response| {
        let target = match response {
            "preset" => ToneTarget::Preset,
            "temp" => ToneTarget::Temp,
            _ => return,
        };
        if let Ok(index) = usize::try_from(dropdown.selected()) {
            on_choice(index, target);
        }
    });
    dialog.present(Some(parent));
}

pub fn ask_tone_name(
    parent: &impl IsA<gtk::Widget>,
    suggestion: &str,
    on_name: impl Fn(String) + 'static,
) {
    let dialog = adw::AlertDialog::new(
        Some(&tr("Save as Local Preset")),
        Some(&tr(
            "The imported preset is stored on this computer. The amp is not changed.",
        )),
    );
    let entry = gtk::Entry::builder()
        .text(suggestion)
        .activates_default(true)
        .placeholder_text(tr("Preset Name"))
        .build();
    entry.update_property(&[gtk::accessible::Property::Label(&tr("Preset Name"))]);
    dialog.set_extra_child(Some(&entry));
    dialog.add_responses(&[("cancel", &tr("Cancel")), ("save", &tr("Save"))]);
    dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("save"));
    dialog.set_close_response("cancel");
    dialog.set_response_enabled("save", !suggestion.trim().is_empty());
    let handler_entry = entry.clone();
    dialog.connect_response(None, move |_, response| {
        let name = handler_entry.text().trim().to_owned();
        if response == "save" && !name.is_empty() {
            on_name(name);
        }
    });
    let sync_dialog = dialog.clone();
    entry.connect_changed(move |entry| {
        sync_dialog.set_response_enabled("save", !entry.text().trim().is_empty());
    });
    dialog.present(Some(parent));
    entry.grab_focus();
}

pub fn choose_export(
    parent: &impl IsA<gtk::Widget>,
    presets: &[PresetEntry],
    limit: usize,
    on_choice: impl Fn(Vec<String>, String) + 'static,
) {
    let dialog = adw::AlertDialog::new(
        Some(&tr("Export Presets")),
        Some(&trf(
            "Choose up to {limit} local presets for a BOSS TONE STUDIO liveset (.tsl).",
            &[("limit", &limit.to_string())],
        )),
    );
    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    let entry = gtk::Entry::builder()
        .text(tr("Tsuba"))
        .placeholder_text(tr("Liveset Name"))
        .build();
    entry.update_property(&[gtk::accessible::Property::Label(&tr("Liveset Name"))]);
    let (scroller, checks) = export_rows(presets);
    let counter = gtk::Label::builder()
        .xalign(0.0)
        .css_classes(["dim-label", "caption"])
        .build();
    content.append(&entry);
    content.append(&scroller);
    content.append(&counter);
    dialog.set_extra_child(Some(&content));
    dialog.add_responses(&[("cancel", &tr("Cancel")), ("export", &tr("Export…"))]);
    dialog.set_response_appearance("export", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("export"));
    dialog.set_close_response("cancel");
    let sync = {
        let checks = Rc::clone(&checks);
        let dialog = dialog.clone();
        let entry = entry.clone();
        move || {
            let chosen = checks
                .borrow()
                .iter()
                .filter(|(_, check)| check.is_active())
                .count();
            for (_, check) in checks.borrow().iter() {
                check.set_sensitive(check.is_active() || chosen < limit);
            }
            counter.set_text(&trf(
                "{count} of {limit} selected",
                &[
                    ("count", &chosen.to_string()),
                    ("limit", &limit.to_string()),
                ],
            ));
            dialog.set_response_enabled(
                "export",
                chosen > 0 && chosen <= limit && !entry.text().trim().is_empty(),
            );
        }
    };
    let sync = Rc::new(sync);
    sync();
    for (_, check) in checks.borrow().iter() {
        let sync = Rc::clone(&sync);
        check.connect_toggled(move |_| sync());
    }
    let entry_sync = Rc::clone(&sync);
    entry.connect_changed(move |_| entry_sync());
    dialog.connect_response(None, move |_, response| {
        if response != "export" {
            return;
        }
        let keys: Vec<String> = checks
            .borrow()
            .iter()
            .filter(|(_, check)| check.is_active())
            .map(|(key, _)| key.clone())
            .collect();
        let name = entry.text().trim().to_owned();
        if !keys.is_empty() && !name.is_empty() {
            on_choice(keys, name);
        }
    });
    dialog.present(Some(parent));
}

type ExportChecks = Rc<RefCell<Vec<(String, gtk::CheckButton)>>>;

fn export_rows(presets: &[PresetEntry]) -> (gtk::ScrolledWindow, ExportChecks) {
    let list = gtk::ListBox::builder()
        .selection_mode(gtk::SelectionMode::None)
        .css_classes(["boxed-list"])
        .build();
    let checks: ExportChecks = Rc::new(RefCell::new(Vec::new()));
    for entry in presets {
        let check = gtk::CheckButton::builder()
            .valign(gtk::Align::Center)
            .build();
        let row = adw::ActionRow::builder()
            .title(&entry.name)
            .subtitle(&entry.patch_name)
            .use_markup(false)
            .activatable_widget(&check)
            .build();
        row.add_prefix(&check);
        list.append(&row);
        checks.borrow_mut().push((entry.key.clone(), check));
    }
    let scroller = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .min_content_height(120)
        .max_content_height(320)
        .propagate_natural_height(true)
        .child(&list)
        .build();
    (scroller, checks)
}

pub fn shortcuts(parent: &impl IsA<gtk::Widget>) {
    let groups: [(String, Vec<(String, &str)>); 4] = [
        (
            tr("General"),
            vec![
                (tr("Keyboard Shortcuts"), "<Control>question"),
                (tr("Close Window"), "<Control>w"),
                (tr("Quit"), "<Control>q"),
            ],
        ),
        (tr("Amp"), vec![(tr("Reload From Amp"), "<Control>r")]),
        (
            tr("Presets"),
            vec![
                (tr("Save Current Sound as Preset"), "<Control>s"),
                (tr("Search Presets"), "<Control>f"),
            ],
        ),
        (
            tr("Signal Chain"),
            vec![(tr("Select Focused Block"), "Return space")],
        ),
    ];
    let dialog = adw::ShortcutsDialog::new();
    for (title, items) in groups {
        let section = adw::ShortcutsSection::new(Some(&title));
        for (label, accelerator) in items {
            section.add(adw::ShortcutsItem::new(&label, accelerator));
        }
        dialog.add(section);
    }
    dialog.present(Some(parent));
}

pub fn about(parent: &impl IsA<gtk::Widget>) {
    let dialog = adw::AboutDialog::builder()
        .application_name(tr("Tsuba"))
        .application_icon(APP_ID)
        .developer_name("Jan Klíma")
        .developers(vec!["Jan Klíma"])
        .version(env!("CARGO_PKG_VERSION"))
        .license_type(gtk::License::Gpl30)
        .comments(tr("Edit the sound of a BOSS Katana MK II amplifier."))
        .build();
    dialog.add_credit_section(Some(&tr("Implementation")), &[&tr("AI-assisted code")]);
    dialog.add_acknowledgement_section(
        Some(&tr("Parameter Maps")),
        &["Colin Willcocks (Katana Mk2 FxFloorBoard)"],
    );
    dialog.add_credit_section(
        Some(&tr("Icons")),
        &[
            "Lucide https://lucide.dev",
            "Phosphor Icons https://phosphoricons.com",
        ],
    );
    dialog.add_legal_section(
        "Katana Mk2 FxFloorBoard",
        None,
        gtk::License::Custom,
        Some(SOURCE_ATTRIBUTION),
    );
    dialog.add_legal_section(
        "Lucide",
        Some("© Lucide Icons and Contributors, © Cole Bemis (Feather)"),
        gtk::License::Custom,
        Some(LUCIDE_LICENSE),
    );
    dialog.add_legal_section(
        "Phosphor Icons",
        Some("© 2023 Phosphor Icons"),
        gtk::License::MitX11,
        None,
    );
    dialog.present(Some(parent));
}
