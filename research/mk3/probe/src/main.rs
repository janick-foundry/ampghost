use std::sync::mpsc;
use std::time::{Duration, Instant};
use midir::{Ignore, MidiInput, MidiOutput};

const HEADER: [u8; 6] = [0xF0, 0x41, 0x10, 0x01, 0x05, 0x07];

fn hex(b: &[u8]) -> String { b.iter().map(|x| format!("{x:02X}")).collect::<Vec<_>>().join(" ") }

fn parse4(s: &str) -> [u8; 4] {
    let v: Vec<u8> = s.split_whitespace().map(|w| u8::from_str_radix(w, 16).unwrap()).collect();
    assert!(v.len() == 4 && v.iter().all(|x| *x < 0x80));
    [v[0], v[1], v[2], v[3]]
}

fn rq1(header: &[u8], addr: [u8; 4], size: [u8; 4]) -> Vec<u8> {
    let mut m = header.to_vec();
    m.push(0x11);
    m.extend_from_slice(&addr);
    m.extend_from_slice(&size);
    let sum: u32 = addr.iter().chain(size.iter()).map(|x| u32::from(*x)).sum();
    m.push(((128 - (sum % 128)) % 128) as u8);
    m.push(0xF7);
    m
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let port_marker = "KATANA3";
    let wait = Duration::from_millis(args.get(3).map_or(1500, |s| s.parse().unwrap()));
    let msg = match args[1].as_str() {
        "identity" => vec![0xF0, 0x7E, 0x7F, 0x06, 0x01, 0xF7],
        "rq1" => rq1(&HEADER, parse4(&args[2]), parse4(&args[4])),
        "rq1mk2" => rq1(&[0xF0, 0x41, 0x00, 0x00, 0x00, 0x00, 0x33], parse4(&args[2]), parse4(&args[4])),
        _ => panic!("only identity and rq1 are allowed"),
    };
    assert!(msg[1] == 0x7E || msg[msg.len() - 11] == 0x11);
    let mut input = MidiInput::new("mk3-probe-in").unwrap();
    input.ignore(Ignore::None);
    let ip = input.ports().into_iter().find(|p| input.port_name(p).unwrap().contains(port_marker)).expect("no KATANA3 input");
    let output = MidiOutput::new("mk3-probe-out").unwrap();
    let op = output.ports().into_iter().find(|p| output.port_name(p).unwrap().contains(port_marker)).expect("no KATANA3 output");
    let (tx, rx) = mpsc::channel::<Vec<u8>>();
    let mut buf = Vec::new();
    let _conn = input.connect(&ip, "mk3-probe", move |_, chunk, _| {
        for &b in chunk {
            if b == 0xF0 { buf.clear(); }
            buf.push(b);
            if b == 0xF7 { let _ = tx.send(std::mem::take(&mut buf)); }
        }
    }, ()).unwrap();
    let mut out = output.connect(&op, "mk3-probe").unwrap();
    std::thread::sleep(Duration::from_millis(50));
    eprintln!("TX {}", hex(&msg));
    out.send(&msg).unwrap();
    let deadline = Instant::now() + wait;
    while let Ok(m) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        println!("{}", hex(&m));
    }
}
