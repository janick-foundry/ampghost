use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

use crate::error::ParamsError;
use crate::param::{Param, RawParam, Status};

const PARAMS_JSON: &str = include_str!("../data/params.json");

pub const SOURCE_ATTRIBUTION: &str = "Parameter maps derived from Katana Mk2 FxFloorBoard by Colin Willcocks (gumtown), source code 20240515, midi.xml v2.02, GPL";

const VERIFIED: [([u8; 4], &str); 5] = [
    ([0x60, 0x00, 0x00, 0x22], "2026-10-22"),
    ([0x60, 0x00, 0x00, 0x24], "2026-10-22"),
    ([0x60, 0x00, 0x00, 0x26], "2026-10-22"),
    ([0x60, 0x00, 0x00, 0x28], "2026-10-22"),
    ([0x60, 0x00, 0x00, 0x43], "2026-10-22"),
];

#[derive(Debug, Deserialize)]
struct RawTable {
    schema: u32,
    params: Vec<RawParam>,
}

#[derive(Debug)]
pub struct ParamTable {
    params: Vec<Param>,
    by_id: HashMap<String, usize>,
}

impl ParamTable {
    fn parse() -> Result<Self, ParamsError> {
        let raw: RawTable = serde_json::from_str(PARAMS_JSON)
            .map_err(|error| ParamsError::Parse(error.to_string()))?;
        if raw.schema != 1 {
            return Err(ParamsError::Parse(format!(
                "unsupported schema {}",
                raw.schema
            )));
        }
        let params = raw
            .params
            .into_iter()
            .map(|param| {
                let status = VERIFIED
                    .iter()
                    .find(|(addr, _)| *addr == param.addr)
                    .map_or(Status::Research, |(_, date)| Status::Verified { date });
                Param::new(param, status)
            })
            .collect::<Vec<_>>();
        let by_id = params
            .iter()
            .enumerate()
            .map(|(index, param)| (param.id().to_owned(), index))
            .collect();
        Ok(Self { params, by_id })
    }

    pub fn get(&self, id: &str) -> Result<&Param, ParamsError> {
        self.by_id
            .get(id)
            .and_then(|&index| self.params.get(index))
            .ok_or_else(|| ParamsError::Unknown(id.to_owned()))
    }

    pub fn iter(&self) -> impl Iterator<Item = &Param> {
        self.params.iter()
    }

    pub fn len(&self) -> usize {
        self.params.len()
    }

    pub fn is_empty(&self) -> bool {
        self.params.is_empty()
    }
}

pub fn table() -> Result<&'static ParamTable, ParamsError> {
    static TABLE: OnceLock<Result<ParamTable, ParamsError>> = OnceLock::new();
    TABLE
        .get_or_init(ParamTable::parse)
        .as_ref()
        .map_err(Clone::clone)
}
