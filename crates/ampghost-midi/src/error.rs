#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MidiError {
    #[cfg(feature = "native")]
    #[error("ALSA sequencer is not available")]
    Alsa(#[from] alsa::Error),
    #[cfg(feature = "native")]
    #[error("MIDI subsystem could not be initialised")]
    Init(#[from] midir::InitError),
    #[error("MIDI port watcher could not be started")]
    Watcher(#[source] std::io::Error),
    #[error("port watching is not available")]
    WatchUnavailable,
    #[error("MIDI port {0} not found")]
    PortNotFound(String),
    #[cfg(feature = "native")]
    #[error("MIDI port {port} could not be opened: {kind}")]
    Connect {
        port: String,
        kind: midir::ConnectErrorKind,
    },
    #[cfg(feature = "native")]
    #[error("sending MIDI failed")]
    Send(#[from] midir::SendError),
    #[error("MIDI port {0} disappeared")]
    PortGone(String),
    #[error("MIDI input closed")]
    Disconnected,
}

impl MidiError {
    pub fn is_disconnect(&self) -> bool {
        matches!(self, Self::PortGone(_) | Self::Disconnected)
    }
}
