#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum MidiError {
    #[error("MIDI subsystem could not be initialised: {0}")]
    Init(String),
    #[error("MIDI port {0} not found")]
    PortNotFound(String),
    #[error("MIDI port {port} could not be opened: {reason}")]
    Connect { port: String, reason: String },
    #[error("sending MIDI failed: {0}")]
    Send(String),
    #[error("MIDI input closed")]
    Disconnected,
}
