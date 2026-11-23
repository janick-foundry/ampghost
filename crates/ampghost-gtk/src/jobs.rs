use std::path::PathBuf;

use ampghost_engine::RequestId;
use ampghost_protocol::{Channel, PatchImage};

use crate::i18n::{tr, trf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FollowUp {
    ApplyPreset { key: String, name: String },
    PutBack(PatchImage),
    SendTone { image: PatchImage, name: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
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
    pub fn shows_status(&self) -> bool {
        !matches!(self, Self::ListPresets)
    }

    pub fn status_text(&self) -> String {
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

#[derive(Debug, Default)]
pub struct Jobs {
    entries: Vec<(RequestId, Job)>,
}

impl Jobs {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn push(&mut self, request: RequestId, job: Job) {
        self.entries.push((request, job));
    }

    pub fn finish(&mut self, request: RequestId) -> Option<Job> {
        let index = self.entries.iter().position(|(id, _)| *id == request)?;
        Some(self.entries.remove(index).1)
    }

    pub fn contains(&self, job: &Job) -> bool {
        self.iter().any(|queued| queued == job)
    }

    pub fn front(&self) -> Option<&Job> {
        self.iter().next()
    }

    pub fn iter(&self) -> impl Iterator<Item = &Job> {
        self.entries.iter().map(|(_, job)| job)
    }
}
