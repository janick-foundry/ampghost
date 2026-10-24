use std::path::PathBuf;

use clap::{Parser, Subcommand};
use katana_protocol::Channel;

#[derive(Debug, Parser)]
#[command(
    name = "katana-cli",
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
        #[arg(value_name = "HEX", required = true)]
        order: Vec<String>,
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
    SwitchChannel {
        channel: Channel,
        #[arg(long)]
        discard_temp: bool,
    },
}
