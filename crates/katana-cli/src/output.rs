use std::fmt::Write as _;

use katana_engine::{DetectReport, WriteOutcome};
use katana_protocol::{Dt1, PatchImage};
use sha2::{Digest, Sha256};

pub fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .fold(String::with_capacity(64), |mut text, byte| {
            let _ = write!(text, "{byte:02x}");
            text
        })
}

pub fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn print_detect(report: &DetectReport) {
    let device = &report.device;
    println!("port:      {}", device.port);
    println!("identity:  {}", device.identity);
    println!("model:     {}", device.model_name());
    if !device.is_supported() {
        println!("status:    unsupported, writes refused (identity information only)");
        return;
    }
    println!("status:    supported");
    if let Some(channel) = report.current_channel {
        println!("channel:   {channel}");
    }
    if let Some(map) = report.program_change_map {
        let entries = map
            .entries()
            .map(|(channel, program)| format!("{channel}={program}"))
            .collect::<Vec<_>>()
            .join(" ");
        println!("pc map:    {entries}");
    }
}

pub fn print_image(image: &PatchImage, syx: &[u8]) {
    println!("patch name: {}", image.name());
    println!("data bytes: {}", image.data().len());
    println!("syx bytes:  {}", syx.len());
    println!("sha256:     {}", sha256_hex(syx));
}

pub fn print_dump(packets: &[Dt1]) {
    for packet in packets {
        for (row, chunk) in packet.data().chunks(16).enumerate() {
            let offset = packet
                .address()
                .offset(u32::try_from(row * 16).unwrap_or_default())
                .map(|address| address.to_string())
                .unwrap_or_default();
            println!("{offset}  {}", hex(chunk));
        }
    }
}

pub fn print_outcome(outcome: &WriteOutcome) {
    if outcome.attempts == 0 {
        println!("verify:     nothing to write, temp already holds the requested values");
        return;
    }
    println!(
        "verify:     OK after {} attempt(s), {} packet(s) sent",
        outcome.attempts, outcome.packets
    );
    if !outcome.report.amp_derived_changes.is_empty() {
        println!(
            "amp-derived bytes changed by the amp (ignored): {}",
            outcome
                .report
                .amp_derived_changes
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !outcome.report.linked_accepted.is_empty() {
        println!(
            "type/variant pairs accepted: {}",
            outcome
                .report
                .linked_accepted
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
}
