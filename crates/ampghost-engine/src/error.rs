use ampghost_midi::MidiError;
use ampghost_params::ParamsError;
use ampghost_protocol::{Address, ByteMismatch, Channel, ProtocolError};
use ampghost_store::StoreError;

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EngineError {
    #[error("no Katana answered the identity request")]
    NotFound {
        #[source]
        last: Option<Box<EngineError>>,
    },
    #[error("no identity reply on {port}")]
    IdentityTimeout { port: String },
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
    #[error("reply to RQ1 {requested} ({expected} bytes) came from {replied} with {actual} bytes")]
    BackupReply {
        requested: Address,
        expected: usize,
        replied: Address,
        actual: usize,
    },
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
