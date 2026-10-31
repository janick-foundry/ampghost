use std::time::Duration;

use crate::error::MidiError;

pub const KATANA_PORT_MARKER: &str = "KATANA";

const PREFERRED_PORT_SUFFIX: &str = "MIDI 1";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PortInfo {
    pub name: String,
}

pub trait MidiTransport: Send {
    fn port_name(&self) -> &str;

    fn send(&mut self, message: &[u8]) -> Result<(), MidiError>;

    fn receive(&mut self, timeout: Duration) -> Result<Option<Vec<u8>>, MidiError>;

    fn is_present(&self) -> bool {
        true
    }
}

pub trait MidiBackend: Send {
    fn ports(&self) -> Result<Vec<PortInfo>, MidiError>;

    fn open(&self, port: &PortInfo) -> Result<Box<dyn MidiTransport>, MidiError>;

    fn watch(&self, notify: Box<dyn Fn() + Send>) -> Result<(), MidiError> {
        drop(notify);
        Err(MidiError::Init("port watching is not available".to_owned()))
    }
}

pub fn katana_candidates(ports: &[PortInfo]) -> Vec<PortInfo> {
    let mut candidates = ports
        .iter()
        .filter(|port| port.name.contains(KATANA_PORT_MARKER))
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by_key(|port| !port.name.contains(PREFERRED_PORT_SUFFIX));
    candidates
}
