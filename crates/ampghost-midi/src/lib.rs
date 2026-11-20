#![forbid(unsafe_code)]

mod assemble;
mod error;
#[cfg(feature = "native")]
mod native;
mod transport;

pub use assemble::SysexAssembler;
pub use error::MidiError;
#[cfg(feature = "native")]
pub use native::MidirBackend;
pub use transport::{KATANA_PORT_MARKER, MidiBackend, MidiTransport, PortInfo, katana_candidates};
