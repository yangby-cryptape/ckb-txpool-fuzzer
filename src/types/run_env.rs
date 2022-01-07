// TODO Add more configurations for running.

use std::{fmt, result::Result as StdResult, str::FromStr};

use ckb_types::core::BlockNumber;
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunEnv {
    pub(crate) chain_blocks: BlockNumber,
    pub(crate) step_interval: u64,
    pub(crate) block_interval: u32,
}

impl FromStr for RunEnv {
    type Err = serde_yaml::Error;
    fn from_str(s: &str) -> StdResult<Self, Self::Err> {
        serde_yaml::from_str(s)
    }
}

impl fmt::Display for RunEnv {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        serde_yaml::to_string(self)
            .map_err(|_| fmt::Error)
            .and_then(|s| write!(f, "{}", s))
    }
}
