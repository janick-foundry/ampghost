use crate::address::{Address, Size};
use crate::error::ProtocolError;
use crate::identity::{IDENTITY_REQUEST, Identity};
use crate::sysex::checksum;

const SYSEX_START: u8 = 0xF0;
const SYSEX_END: u8 = 0xF7;
const ROLAND: u8 = 0x41;
const GEN3_HEADER: [u8; 6] = [SYSEX_START, ROLAND, 0x10, 0x01, 0x05, 0x07];
const COMMAND_RQ1: u8 = 0x11;
const COMMAND_DT1: u8 = 0x12;
const RQ1_LEN: usize = GEN3_HEADER.len() + 1 + 4 + 4 + 2;

#[derive(Debug, PartialEq, Eq)]
pub struct Gen3Model {
    pub name: &'static str,
    pub family: [u8; 2],
}

impl Gen3Model {
    pub fn matches(&self, identity: &Identity) -> bool {
        identity.manufacturer() == ROLAND && identity.family() == self.family
    }
}

pub static KATANA_MK3: Gen3Model = Gen3Model {
    name: "BOSS Katana MK III",
    family: [0x07, 0x05],
};

pub fn gen3_model_for(identity: &Identity) -> Option<&'static Gen3Model> {
    KATANA_MK3.matches(identity).then_some(&KATANA_MK3)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Gen3Rq1 {
    address: Address,
    size: Size,
}

impl Gen3Rq1 {
    pub const fn new(address: Address, size: u8) -> Self {
        Self {
            address,
            size: Size::from_parts(0, 0, 0, size),
        }
    }

    pub const fn address(&self) -> Address {
        self.address
    }

    pub const fn size(&self) -> Size {
        self.size
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut body = Vec::with_capacity(8);
        body.extend_from_slice(&self.address.bytes());
        body.extend_from_slice(&self.size.bytes());
        let mut out = Vec::with_capacity(RQ1_LEN);
        out.extend_from_slice(&GEN3_HEADER);
        out.push(COMMAND_RQ1);
        out.extend_from_slice(&body);
        out.push(checksum(&body));
        out.push(SYSEX_END);
        out
    }
}

pub fn gen3_read_only_allows(message: &[u8]) -> bool {
    message == IDENTITY_REQUEST
        || (message.len() == RQ1_LEN
            && message.starts_with(&GEN3_HEADER)
            && message.get(GEN3_HEADER.len()) == Some(&COMMAND_RQ1)
            && message.last() == Some(&SYSEX_END))
}

pub fn decode_gen3_data(message: &[u8]) -> Result<Option<(Address, Vec<u8>)>, ProtocolError> {
    if !message.starts_with(&GEN3_HEADER) || message.get(GEN3_HEADER.len()) != Some(&COMMAND_DT1) {
        return Ok(None);
    }
    if message.last() != Some(&SYSEX_END) || message.len() < GEN3_HEADER.len() + 1 + 4 + 3 {
        return Err(ProtocolError::BadFrame);
    }
    let body_end = message.len() - 2;
    let body = message
        .get(GEN3_HEADER.len() + 1..body_end)
        .ok_or(ProtocolError::BadFrame)?;
    let received = message.get(body_end).copied().unwrap_or_default();
    let (address_bytes, data) = body.split_at_checked(4).ok_or(ProtocolError::BadFrame)?;
    let address = Address::from_bytes(address_bytes)?;
    let expected = checksum(body);
    if expected != received {
        return Err(ProtocolError::Checksum {
            address,
            expected,
            actual: received,
        });
    }
    Ok(Some((address, data.to_vec())))
}
