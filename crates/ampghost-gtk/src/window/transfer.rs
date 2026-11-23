use std::path::{Path, PathBuf};
use std::rc::Rc;

use adw::prelude::*;
use ampghost_engine::Command;
use ampghost_protocol::PatchImage;
use ampghost_store::{FullBackup, TSL_MAX_PATCHES};
use anyhow::Result;
use gtk::{gio, glib};

use crate::dialogs::{self, ToneTarget};
use crate::files::{self, ToneFile};
use crate::i18n::{tr, trf};
use crate::jobs::{FollowUp, Job};

use super::Ui;

pub(super) fn display_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |name| name.to_string_lossy().into_owned(),
    )
}

impl Ui {
    pub(super) fn timestamp() -> String {
        glib::DateTime::now_local()
            .and_then(|now| now.format("%Y%m%d-%H%M%S"))
            .map_or_else(|_| "now".to_owned(), |text| text.to_string())
    }

    pub(super) fn backup_then(&self, follow_up: FollowUp) {
        self.flush_writes_now();
        let reason = match follow_up {
            FollowUp::ApplyPreset { .. } => "preset",
            FollowUp::PutBack(_) => "put-back",
            FollowUp::SendTone { .. } => "import",
        };
        let key = format!("auto-before-{reason}-{}.syx", Self::timestamp());
        self.send(Command::BackupTemp { key }, Job::Backup(follow_up));
    }

    pub(super) fn run_follow_up(&self, follow_up: FollowUp) {
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

    pub(super) fn file_filter(name: &str, suffix: &str) -> (gio::ListStore, gtk::FileFilter) {
        let filter = gtk::FileFilter::new();
        filter.set_name(Some(name));
        filter.add_suffix(suffix);
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&filter);
        (filters, filter)
    }

    pub(super) fn save_dialog(
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

    pub(super) fn run_blocking<T: Send + 'static>(
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

    pub(super) fn choose_backup_file(self: &Rc<Self>) {
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

    pub(super) fn write_backup(self: &Rc<Self>, path: PathBuf, backup: FullBackup) {
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

    pub(super) fn choose_tone_file(self: &Rc<Self>) {
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

    pub(super) fn load_tone_file(self: &Rc<Self>, path: PathBuf) {
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

    pub(super) fn prompt_tone(self: &Rc<Self>, file: ToneFile) {
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

    pub(super) fn prompt_tone_name(self: &Rc<Self>, image: PatchImage, suggestion: &str) {
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

    pub(super) fn prompt_export(self: &Rc<Self>) {
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

    pub(super) fn write_liveset(self: &Rc<Self>, path: PathBuf, text: String, patches: usize) {
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
}
