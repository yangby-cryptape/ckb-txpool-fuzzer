use crate::{
    config::{InitConfig, RunConfig},
    error::Result,
    fuzzer::Fuzzer,
};

impl InitConfig {
    pub(crate) fn execute(self) -> Result<()> {
        log::info!("Init ...");
        Fuzzer::init(self)
    }
}

impl RunConfig {
    pub(crate) fn execute(self) -> Result<()> {
        log::info!("Run ...");
        Fuzzer::load(self)?.run()
    }
}
