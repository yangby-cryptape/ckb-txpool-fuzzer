use std::{fmt, result::Result as StdResult, str::FromStr};

pub(crate) use ckb_chain_spec::Params;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct MetaData {
    pub(crate) chain_spec: ChainSpec,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct ChainSpec {
    pub(crate) genesis: Genesis,
    pub(crate) params: Params,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct Genesis {
    pub(crate) timestamp: u64,
    pub(crate) compact_target: u32,
}

impl FromStr for MetaData {
    type Err = serde_yaml::Error;
    fn from_str(s: &str) -> StdResult<Self, Self::Err> {
        serde_yaml::from_str(s)
    }
}

impl fmt::Display for MetaData {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        serde_yaml::to_string(self)
            .map_err(|_| fmt::Error)
            .and_then(|s| write!(f, "{}", s))
    }
}
