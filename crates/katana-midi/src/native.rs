use std::sync::mpsc;
use std::time::Duration;

use midir::{Ignore, MidiInput, MidiInputConnection, MidiOutput, MidiOutputConnection};

use crate::assemble::SysexAssembler;
use crate::error::MidiError;
use crate::transport::{MidiBackend, MidiTransport, PortInfo};

const CLIENT_NAME: &str = "katana-control";

#[derive(Debug, Default)]
pub struct MidirBackend;

impl MidirBackend {
    pub fn new() -> Self {
        Self
    }
}

impl MidiBackend for MidirBackend {
    fn ports(&self) -> Result<Vec<PortInfo>, MidiError> {
        let input =
            MidiInput::new(CLIENT_NAME).map_err(|error| MidiError::Init(error.to_string()))?;
        let output =
            MidiOutput::new(CLIENT_NAME).map_err(|error| MidiError::Init(error.to_string()))?;
        let outputs = output
            .ports()
            .iter()
            .filter_map(|port| output.port_name(port).ok())
            .collect::<Vec<_>>();
        let ports = input
            .ports()
            .iter()
            .filter_map(|port| input.port_name(port).ok())
            .filter(|name| outputs.contains(name))
            .map(|name| PortInfo { name })
            .collect();
        Ok(ports)
    }

    fn open(&self, port: &PortInfo) -> Result<Box<dyn MidiTransport>, MidiError> {
        let (sender, receiver) = mpsc::channel();
        let input = connect_input(&port.name, sender)?;
        let output = connect_output(&port.name)?;
        tracing::debug!(port = %port.name, "opened MIDI port");
        Ok(Box::new(MidirTransport {
            name: port.name.clone(),
            _input: input,
            output,
            receiver,
        }))
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
}

impl MidiTransport for MidirTransport {
    fn port_name(&self) -> &str {
        &self.name
    }

    fn send(&mut self, message: &[u8]) -> Result<(), MidiError> {
        tracing::trace!(bytes = message.len(), "midi out");
        self.output
            .send(message)
            .map_err(|error| MidiError::Send(error.to_string()))
    }

    fn receive(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, MidiError> {
        match self.receiver.recv_timeout(timeout) {
            Ok(message) => Ok(Some(message)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(MidiError::Disconnected),
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
    let mut input =
        MidiInput::new(CLIENT_NAME).map_err(|error| MidiError::Init(error.to_string()))?;
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
            reason: error.to_string(),
        })
}

fn connect_output(name: &str) -> Result<MidiOutputConnection, MidiError> {
    let output =
        MidiOutput::new(CLIENT_NAME).map_err(|error| MidiError::Init(error.to_string()))?;
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
            reason: error.to_string(),
        })
}
