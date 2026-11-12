const SYSEX_START: u8 = 0xF0;
const SYSEX_END: u8 = 0xF7;
const SYSEX_LIMIT: usize = 64 * 1024;

#[derive(Debug, Default)]
pub struct SysexAssembler {
    pending: Option<Vec<u8>>,
}

impl SysexAssembler {
    pub fn push(&mut self, chunk: &[u8], mut emit: impl FnMut(Vec<u8>)) {
        if self.pending.is_none() && chunk.first().is_some_and(|&byte| byte != SYSEX_START) {
            emit(chunk.to_vec());
            return;
        }
        for &byte in chunk {
            if byte == SYSEX_START {
                self.pending = Some(vec![byte]);
                continue;
            }
            let Some(buffer) = self.pending.as_mut() else {
                continue;
            };
            buffer.push(byte);
            if byte == SYSEX_END {
                if let Some(message) = self.pending.take() {
                    emit(message);
                }
            } else if buffer.len() > SYSEX_LIMIT {
                tracing::warn!("dropping oversized SysEx");
                self.pending = None;
            }
        }
    }
}
