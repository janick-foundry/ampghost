use katana_protocol::{Channel, Dt1, Identity, PatchImage, ProgramChangeMap, VerifyReport};
use katana_store::FullBackup;

use crate::error::EngineError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamWrite {
    pub id: String,
    pub value: u16,
}

#[derive(Debug)]
pub enum Command {
    Detect,
    ReadTemp,
    WriteTemp(PatchImage),
    WriteParams(Vec<ParamWrite>),
    WriteChain(u8),
    SwitchChannel(Channel),
    BackupTemp { key: String },
    ReadFullBackup,
    RestoreBackup { key: String },
    SavePreset { name: String },
    SaveImportedPreset { name: String, image: PatchImage },
    ExportLiveset { keys: Vec<String>, name: String },
    ApplyPreset { key: String },
    UpdatePreset { key: String, name: String },
    DeletePreset { key: String },
    ListPresets,
    ListBackups,
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub port: String,
    pub identity: Identity,
    pub profile: Option<&'static katana_protocol::DeviceProfile>,
}

impl DeviceInfo {
    pub fn model_name(&self) -> String {
        self.profile.map_or_else(
            || format!("unknown device {}", self.identity.hex()),
            |profile| profile.name.to_owned(),
        )
    }

    pub fn is_supported(&self) -> bool {
        self.profile.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DetectReport {
    pub device: DeviceInfo,
    pub current_channel: Option<Channel>,
    pub program_change_map: Option<ProgramChangeMap>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WriteOutcome {
    pub attempts: u8,
    pub packets: usize,
    pub report: VerifyReport,
    pub image: PatchImage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChainOutcome {
    pub previous: u8,
    pub requested: u8,
    pub actual: u8,
    pub order: Vec<u8>,
    pub packets: usize,
    pub report: VerifyReport,
    pub image: PatchImage,
}

impl ChainOutcome {
    pub fn is_normalised(&self) -> bool {
        self.actual != self.requested
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchOutcome {
    pub channel: Channel,
    pub program: u8,
    pub attempts: u8,
    pub temp: PatchImage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PresetEntry {
    pub key: String,
    pub name: String,
    pub model: String,
    pub created: String,
    pub patch_name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    EditorMode,
    Writing { packets: usize },
    Verifying { attempt: u8 },
    Rewriting { bytes: usize },
    WaitingForAmp { millis: u64 },
    ProgramChange { program: u8, attempt: u8 },
    ConfirmingChannel { attempt: u8 },
    ReadingTemp,
    ReadingBackup { done: usize, total: usize },
}

#[derive(Debug, Clone)]
pub struct AmpSwitch {
    pub channel: Channel,
    pub temp: PatchImage,
    pub from_dump: bool,
}

#[derive(Debug)]
pub enum Event {
    Detected(DetectReport),
    TempRead(PatchImage),
    Progress(Stage),
    WriteVerified(WriteOutcome),
    ChainWritten(ChainOutcome),
    ChannelSwitched(SwitchOutcome),
    BackupSaved {
        key: String,
        image: PatchImage,
    },
    FullBackupRead(FullBackup),
    Restored {
        key: String,
        outcome: WriteOutcome,
    },
    PresetSaved {
        key: String,
        name: String,
    },
    PresetUpdated {
        previous: String,
        key: String,
        name: String,
    },
    PresetDeleted {
        key: String,
    },
    PresetApplied {
        key: String,
        name: String,
        outcome: WriteOutcome,
    },
    Presets(Vec<PresetEntry>),
    LivesetEncoded {
        text: String,
        patches: usize,
    },
    Backups(Vec<String>),
    Unsolicited(Dt1),
    AmpSwitched(AmpSwitch),
    Disconnected {
        port: String,
    },
    Reconnected(DetectReport),
    Failed(EngineError),
    ShutDown,
}

impl Event {
    pub fn is_terminal(&self) -> bool {
        !matches!(
            self,
            Self::Progress(_)
                | Self::Unsolicited(_)
                | Self::AmpSwitched(_)
                | Self::Disconnected { .. }
                | Self::Reconnected(_)
        )
    }
}
