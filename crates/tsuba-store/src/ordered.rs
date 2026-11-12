use std::collections::BTreeMap;

use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use crate::blocks::{NamedBlock, hex_list, unhex_list};
use crate::error::StoreError;

pub(crate) struct OrderedBlocks<'a> {
    pub(crate) prefix: &'static str,
    pub(crate) blocks: &'a [NamedBlock],
}

impl Serialize for OrderedBlocks<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.blocks.len()))?;
        for block in self.blocks {
            map.serialize_entry(
                &format!("{}%{}", self.prefix, block.name),
                &hex_list(&block.data),
            )?;
        }
        map.end()
    }
}

pub(crate) fn named_blocks(
    prefix: &str,
    entries: BTreeMap<String, Vec<String>>,
) -> Result<Vec<NamedBlock>, StoreError> {
    entries
        .into_iter()
        .map(|(key, values)| {
            let name = key
                .strip_prefix(prefix)
                .and_then(|rest| rest.strip_prefix('%'))
                .map_or_else(|| key.clone(), ToOwned::to_owned);
            let data = unhex_list(&key, &values)?;
            Ok(NamedBlock { name, data })
        })
        .collect()
}
