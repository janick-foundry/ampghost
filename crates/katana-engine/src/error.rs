use katana_midi::MidiError;
use katana_params::ParamsError;
use katana_protocol::{Address, ByteMismatch, Channel, ProtocolError};
use katana_store::StoreError;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EngineError {
    #[error("no Katana answered the identity request{}", last_reason(.last.as_deref()))]
    NotFound { last: Option<String> },
    #[error(
        "unsupported device ({identity}); only identity information is available and all writes are refused"
    )]
    Unsupported { identity: String },
    #[error(transparent)]
    Midi(#[from] MidiError),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Params(#[from] ParamsError),
    #[error("timeout waiting for the reply to RQ1 {address} ({received} packets received)")]
    Timeout { address: Address, received: usize },
    #[error("verify failed at {} bytes: {}", .mismatches.len(), describe(.mismatches))]
    VerifyFailed { mismatches: Vec<ByteMismatch> },
    #[error("channel {0} is not available on this model")]
    ChannelUnavailable(Channel),
    #[error("switching to {wanted} failed, amp reports {}", actual.map_or_else(|| "nothing".to_owned(), |channel| channel.to_string()))]
    ChannelSelectFailed {
        wanted: Channel,
        actual: Option<Channel>,
    },
    #[error("stored preset is not valid UTF-8")]
    PresetEncoding,
    #[error("the Katana was disconnected")]
    Disconnected,
    #[error("engine thread is not running")]
    Stopped,
}

impl EngineError {
    pub fn is_disconnect(&self) -> bool {
        match self {
            Self::Disconnected => true,
            Self::Midi(error) => error.is_disconnect(),
            _ => false,
        }
    }
}

fn last_reason(last: Option<&str>) -> String {
    last.map(|reason| format!(" (last error: {reason})"))
        .unwrap_or_default()
}

fn describe(mismatches: &[ByteMismatch]) -> String {
    let mut text = mismatches
        .iter()
        .take(16)
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    if mismatches.len() > 16 {
        text.push_str(", ...");
    }
    text
}
