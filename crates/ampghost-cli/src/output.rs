use std::fmt::Write as _;

use ampghost_engine::{ChainOutcome, DetectReport, WriteOutcome};
use ampghost_protocol::{Dt1, KATANA_50_MK2, PatchImage};

pub fn sha256_hex(bytes: &[u8]) -> String {
    crate::sha256::digest(bytes)
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
                .map_or_else(|_| String::new(), |address| address.to_string());
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

pub fn print_chain_outcome(outcome: &ChainOutcome) {
    println!("pattern was: {}", outcome.previous);
    println!("requested:   {}", outcome.requested);
    if outcome.packets == 0 {
        println!("chain:       nothing to write, temp already holds this pattern");
        return;
    }
    println!("read back:   {} ({})", outcome.actual, hex(&outcome.order));
    if outcome.is_normalised() {
        println!("chain:       the amp did not keep the pattern (not retried)");
    } else {
        println!("chain:       kept, {} packet(s) sent", outcome.packets);
    }
    let other = outcome
        .report
        .mismatches
        .iter()
        .filter(|mismatch| {
            !KATANA_50_MK2
                .write_order
                .effect_chain
                .contains(mismatch.offset)
        })
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if !other.is_empty() {
        println!("other bytes changed by the amp: {}", other.join(", "));
    }
}
