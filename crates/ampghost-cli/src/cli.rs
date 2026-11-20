use std::path::PathBuf;

use ampghost_protocol::Channel;
use clap::{Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(
    name = "ampghost-cli",
    version,
    about = "Internal diagnostic tool for the BOSS Katana MK II"
)]
pub struct Args {
    #[arg(long, global = true, value_name = "DIR")]
    pub data_dir: Option<PathBuf>,
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    pub verbose: u8,
    #[command(subcommand)]
    pub command: Action,
}

#[derive(Debug, Subcommand)]
pub enum Action {
    Detect,
    ReadTemp {
        #[arg(long)]
        dump: bool,
    },
    Backup {
        #[arg(long, value_name = "FILE.syx")]
        out: Option<PathBuf>,
    },
    Restore {
        #[arg(value_name = "FILE.syx")]
        file: PathBuf,
    },
    WriteTemp {
        #[arg(value_name = "FILE.syx")]
        file: PathBuf,
    },
    WriteChain {
        #[arg(value_name = "PATTERN", value_parser = clap::value_parser!(u8).range(0..=6))]
        pattern: u8,
    },
    ReadParam {
        id: String,
    },
    WriteParam {
        id: String,
        value: u16,
    },
    PresetSave {
        name: String,
    },
    PresetApply {
        name: String,
    },
    PresetList,
    PresetDerive {
        from: String,
        #[arg(long)]
        name: String,
        #[arg(long, value_name = "TEXT")]
        patch_name: Option<String>,
        #[arg(long = "set", value_name = "ID=VALUE")]
        sets: Vec<String>,
        #[arg(long)]
        replace: bool,
    },
    BackupAll {
        #[arg(long, value_name = "FILE.alb")]
        out: Option<PathBuf>,
        #[arg(long)]
        replace: bool,
        #[arg(long)]
        plan: bool,
    },
    AlbInfo {
        #[arg(value_name = "FILE.alb")]
        file: PathBuf,
    },
    TslExport {
        #[arg(long, value_name = "FILE.tsl")]
        out: PathBuf,
        #[arg(long, value_name = "LIVESET")]
        name: Option<String>,
        #[arg(long, conflicts_with = "presets")]
        from_temp: bool,
        #[arg(long)]
        replace: bool,
        #[arg(value_name = "PRESET")]
        presets: Vec<String>,
    },
    TslImport {
        #[arg(value_name = "FILE.tsl")]
        file: PathBuf,
        #[arg(long)]
        index: Option<usize>,
        #[arg(long, conflicts_with_all = ["name", "replace", "dry_run"])]
        to_temp: bool,
        #[arg(long)]
        name: Option<String>,
        #[arg(long)]
        replace: bool,
        #[arg(long)]
        dry_run: bool,
    },
    SwitchChannel {
        channel: Channel,
        #[arg(long)]
        discard_temp: bool,
    },
}
