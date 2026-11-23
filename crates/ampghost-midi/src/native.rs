use std::collections::BTreeSet;
use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError, Weak, mpsc};
use std::thread;
use std::time::{Duration, Instant};

use alsa::poll::Descriptors;
use alsa::seq::{Addr, ClientIter, EventType, PortCap, PortIter, PortSubscribe, PortType, Seq};
use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};

use crate::assemble::SysexAssembler;
use crate::error::MidiError;
use crate::transport::{MidiBackend, MidiTransport, PortInfo, PortWatch};

const CLIENT_NAME: &str = "ampghost";
const WATCH_CLIENT_NAME: &CStr = c"ampghost-watch";
const ANNOUNCE_PORT_NAME: &CStr = c"announce";
const RECEIVE_SLICE: Duration = Duration::from_millis(50);
const WATCH_SLICE_MILLIS: i32 = 200;

type Registry = Arc<Mutex<Vec<(String, Weak<AtomicBool>)>>>;

#[derive(Debug, Default)]
pub struct MidirBackend {
    open: Registry,
}

impl MidirBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

fn port_names(seq: &Seq) -> BTreeSet<String> {
    let wanted = PortCap::READ | PortCap::SUBS_READ | PortCap::WRITE | PortCap::SUBS_WRITE;
    let kinds = PortType::MIDI_GENERIC | PortType::SYNTH | PortType::APPLICATION;
    let mut names = BTreeSet::new();
    for client in ClientIter::new(seq) {
        let Ok(client_name) = client.get_name().map(str::to_owned) else {
            continue;
        };
        for port in PortIter::new(seq, client.get_client()) {
            if !port.get_type().intersects(kinds) || !port.get_capability().contains(wanted) {
                continue;
            }
            if let Ok(port_name) = port.get_name() {
                names.insert(format!(
                    "{client_name}:{port_name} {}:{}",
                    port.get_client(),
                    port.get_port()
                ));
            }
        }
    }
    names
}

fn mark_gone(open: &Registry, present: &BTreeSet<String>) {
    let mut entries = open.lock().unwrap_or_else(PoisonError::into_inner);
    entries.retain(|(name, flag)| {
        let Some(flag) = flag.upgrade() else {
            return false;
        };
        if !present.contains(name) && !flag.swap(true, Ordering::SeqCst) {
            tracing::info!(port = %name, "MIDI port disappeared");
        }
        true
    });
}

fn announce_seq() -> Result<Seq, MidiError> {
    let seq = Seq::open(None, Some(alsa::Direction::Capture), false)?;
    seq.set_client_name(WATCH_CLIENT_NAME)?;
    let port = seq.create_simple_port(
        ANNOUNCE_PORT_NAME,
        PortCap::WRITE | PortCap::SUBS_WRITE | PortCap::NO_EXPORT,
        PortType::APPLICATION,
    )?;
    let subscription = PortSubscribe::empty()?;
    subscription.set_sender(Addr::system_announce());
    subscription.set_dest(Addr {
        client: seq.client_id()?,
        port,
    });
    seq.subscribe_port(&subscription)?;
    Ok(seq)
}

fn announced(seq: &Seq) -> Result<bool, alsa::Error> {
    let mut fds = (seq, Some(alsa::Direction::Capture)).get()?;
    if alsa::poll::poll(&mut fds, WATCH_SLICE_MILLIS)? == 0 {
        return Ok(false);
    }
    let mut input = seq.input();
    let mut relevant = false;
    while input.event_input_pending(true)? > 0 {
        let event = input.event_input()?;
        relevant |= matches!(
            event.get_type(),
            EventType::PortStart
                | EventType::PortExit
                | EventType::PortChange
                | EventType::ClientStart
                | EventType::ClientExit
        );
    }
    Ok(relevant)
}

fn watch_loop(seq: &Seq, open: &Registry, notify: &dyn Fn(), stop: &AtomicBool) {
    let mut known = port_names(seq);
    let mut failures = 0_u32;
    while !stop.load(Ordering::SeqCst) {
        let relevant = match announced(seq) {
            Ok(relevant) => relevant,
            Err(error) => {
                failures += 1;
                if failures > 20 {
                    tracing::warn!(
                        error = &error as &dyn std::error::Error,
                        "MIDI port watch stopped"
                    );
                    return;
                }
                tracing::debug!(
                    error = &error as &dyn std::error::Error,
                    "MIDI announce read failed"
                );
                thread::sleep(Duration::from_millis(100));
                true
            }
        };
        if !relevant {
            continue;
        }
        let present = port_names(seq);
        if present == known {
            continue;
        }
        tracing::info!(ports = present.len(), "MIDI ports changed");
        mark_gone(open, &present);
        known = present;
        failures = 0;
        notify();
    }
}

impl MidiBackend for MidirBackend {
    fn ports(&self) -> Result<Vec<PortInfo>, MidiError> {
        let seq = Seq::open(None, None, true)?;
        let present = port_names(&seq);
        mark_gone(&self.open, &present);
        Ok(present.into_iter().map(|name| PortInfo { name }).collect())
    }

    fn open(&self, port: &PortInfo) -> Result<Box<dyn MidiTransport>, MidiError> {
        let (sender, receiver) = mpsc::channel();
        let input = connect_input(&port.name, sender)?;
        let output = connect_output(&port.name)?;
        let gone = Arc::new(AtomicBool::new(false));
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((port.name.clone(), Arc::downgrade(&gone)));
        tracing::debug!(port = %port.name, "opened MIDI port");
        Ok(Box::new(MidirTransport {
            name: port.name.clone(),
            _input: input,
            output,
            receiver,
            gone,
        }))
    }

    fn watch(&self, notify: Box<dyn Fn() + Send>) -> Result<PortWatch, MidiError> {
        let seq = announce_seq()?;
        let open = Arc::clone(&self.open);
        let stop = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("ampghost-port-watch".to_owned())
            .spawn(move || watch_loop(&seq, &open, &*notify, &stopped))
            .map_err(MidiError::Watcher)?;
        tracing::info!("watching MIDI port announcements");
        Ok(PortWatch::new(stop, thread))
    }
}

struct InputState {
    sender: mpsc::Sender<Vec<u8>>,
    assembler: SysexAssembler,
}

struct MidirTransport {
    name: String,
    _input: MidiInputConnection<InputState>,
    output: MidiOutputConnection,
    receiver: mpsc::Receiver<Vec<u8>>,
    gone: Arc<AtomicBool>,
}

impl MidirTransport {
    fn check_present(&self) -> Result<(), MidiError> {
        if self.gone.load(Ordering::SeqCst) {
            Err(MidiError::PortGone(self.name.clone()))
        } else {
            Ok(())
        }
    }
}

impl MidiTransport for MidirTransport {
    fn port_name(&self) -> &str {
        &self.name
    }

    fn is_present(&self) -> bool {
        !self.gone.load(Ordering::SeqCst)
    }

    fn send(&mut self, message: &[u8]) -> Result<(), MidiError> {
        self.check_present()?;
        tracing::trace!(bytes = ?message, "midi out");
        self.output.send(message).map_err(MidiError::from)
    }

    fn receive(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, MidiError> {
        let deadline = Instant::now() + timeout;
        loop {
            self.check_present()?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.receiver.recv_timeout(remaining.min(RECEIVE_SLICE)) {
                Ok(message) => return Ok(Some(message)),
                Err(mpsc::RecvTimeoutError::Timeout) if remaining <= RECEIVE_SLICE => {
                    return Ok(None);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err(MidiError::Disconnected),
            }
        }
    }
}
fn on_input(_stamp: u64, chunk: &[u8], state: &mut InputState) {
    let sender = &state.sender;
    state.assembler.push(chunk, |message| {
        if sender.send(message).is_err() {
            tracing::debug!("MIDI receiver gone");
        }
    });
}

fn connect_input(
    name: &str,
    sender: mpsc::Sender<Vec<u8>>,
) -> Result<MidiInputConnection<InputState>, MidiError> {
    let mut input = MidiInput::new(CLIENT_NAME)?;
    input.ignore(Ignore::None);
    let port = input
        .ports()
        .into_iter()
        .find(|port| {
            input
                .port_name(port)
                .is_ok_and(|candidate| candidate == name)
        })
        .ok_or_else(|| MidiError::PortNotFound(name.to_owned()))?;
    let state = InputState {
        sender,
        assembler: SysexAssembler::default(),
    };
    input
        .connect(&port, CLIENT_NAME, on_input, state)
        .map_err(|error| MidiError::Connect {
            port: name.to_owned(),
            kind: error.kind(),
        })
}

fn connect_output(name: &str) -> Result<MidiOutputConnection, MidiError> {
    let output = MidiOutput::new(CLIENT_NAME)?;
    let port = output
        .ports()
        .into_iter()
        .find(|port| {
            output
                .port_name(port)
                .is_ok_and(|candidate| candidate == name)
        })
        .ok_or_else(|| MidiError::PortNotFound(name.to_owned()))?;
    output
        .connect(&port, CLIENT_NAME)
        .map_err(|error| MidiError::Connect {
            port: name.to_owned(),
            kind: error.kind(),
        })
}
