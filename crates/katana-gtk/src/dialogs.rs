use adw::prelude::*;
use katana_engine::APP_ID;
use katana_params::SOURCE_ATTRIBUTION;
use katana_protocol::Channel;

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

pub fn choose_backup(
    parent: &impl IsA<gtk::Widget>,
    keys: Vec<String>,
    on_key: impl Fn(String) + 'static,
) {
    let dialog = adw::AlertDialog::new(
        Some(&tr("Restore Backup to Temp?")),
        Some(&tr(
            "The chosen backup replaces the current sound in the amp’s temporary patch. The current sound is backed up first. Amp channels are not changed.",
        )),
    );
    let labels: Vec<&str> = keys.iter().map(String::as_str).collect();
    let dropdown = gtk::DropDown::from_strings(&labels);
    dropdown.update_property(&[gtk::accessible::Property::Label(&tr("Backup"))]);
    dialog.set_extra_child(Some(&dropdown));
    dialog.add_responses(&[("cancel", &tr("Cancel")), ("restore", &tr("Restore"))]);
    dialog.set_response_appearance("restore", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.connect_response(None, move |_, response| {
        let chosen = usize::try_from(dropdown.selected())
            .ok()
            .and_then(|index| keys.get(index));
        if response == "restore"
            && let Some(key) = chosen
        {
            on_key(key.clone());
        }
    });
    dialog.present(Some(parent));
}

pub fn about(parent: &impl IsA<gtk::Widget>) {
    let dialog = adw::AboutDialog::builder()
        .application_name(tr("Katana Control"))
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
