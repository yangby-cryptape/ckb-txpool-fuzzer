use std::{process, sync::atomic::Ordering, thread, time};

use ckb_types::packed;

use crate::{
    config::{InitConfig, RunConfig},
    error::Result,
    types::RandomGenerator,
    utils,
};

mod mocked_chain;
mod mocked_store;
mod overlay;
mod storage;
mod strategy;

pub(crate) use mocked_chain::MockedChain;
pub(crate) use mocked_store::MockedStore;
pub(crate) use overlay::{Overlay, TxOverlay, TxOverlayChanges};
pub(crate) use storage::Storage;

pub(crate) struct Fuzzer {
    chain: MockedChain,
    config: RunConfig,
}

impl Fuzzer {
    pub(crate) fn init(cfg: InitConfig) -> Result<()> {
        MockedChain::init(&cfg.data_dir, &cfg.meta_data.chain_spec)?;
        cfg.storage.put_meta_data(&cfg.meta_data)?;
        Ok(())
    }

    pub(crate) fn load(cfg: RunConfig) -> Result<Self> {
        let meta_data = cfg.storage.get_meta_data()?;
        utils::faketime::enable()?;
        let chain = MockedChain::load(&cfg.data_dir, &meta_data.chain_spec)?;
        Ok(Self { chain, config: cfg })
    }

    pub(crate) fn run(self) -> Result<()> {
        let Self { mut chain, config } = self;
        let RunConfig {
            data_dir: _,
            storage,
            run_env,
        } = config;

        let tip_header = chain.chain_tip_header();
        let tip_timestamp = tip_header.timestamp();
        utils::faketime::update(tip_timestamp)?;

        let start_number = tip_header.number();

        let random_generator = RandomGenerator::new(&run_env)?;

        let ctrlc_pressed = utils::ctrlc::capture()?;

        // Run randomly.
        while !ctrlc_pressed.load(Ordering::SeqCst) {
            utils::faketime::increase(random_generator.block_interval())?;

            let txs = strategy::build_transactions(&random_generator, &chain, &storage)?;
            log::trace!("[SendTxs] try to send transactions");
            for tx in &txs {
                let tx_view = tx.view();
                let tx_hash = tx_view.hash();
                let changes = tx.changes();
                let result = chain.txpool_submit_local_tx(tx_view);
                match (changes, result) {
                    (Ok((tx_status, updates)), Ok(())) => {
                        log::info!("[SendTxs] >>> send {:#x} passed", tx_hash);
                        storage.submit_tx(tx_view, tx_status, updates)?;
                    }
                    (Err(updates), Err(_)) => {
                        log::info!("[SendTxs] >>> send {:#x} failed", tx_hash);
                        storage.submit_invalid_tx(tx_view)?;
                        for (tx_hash, tx_status) in updates {
                            storage.remove_invalid_tx(&tx_hash, &tx_status)?;
                        }
                    }
                    (Ok(_), Err(errmsg)) => {
                        log::error!(
                            "[SendTxs] >>> send {:#x} expect passed but got {}",
                            tx_hash,
                            errmsg
                        );
                        process::exit(1);
                    }
                    (Err(_), Ok(())) => {
                        log::warn!("[SendTxs] >>> send {:#x} expect failed but passed", tx_hash);
                    }
                };
            }

            let block_template = chain.get_block_template()?;

            let block: packed::Block = block_template.into();
            let block_view = block.into_view();
            log::trace!(
                "new block: num: {}, ts: {}, txs: {}, proposals: {}",
                block_view.number(),
                block_view.timestamp(),
                block_view.transactions().len(),
                block_view.data().proposals().len(),
            );

            chain.chain_submit_block(&block_view);
            chain.txpool_submit_block(&block_view)?;
            storage.confirm_block(&block_view)?;

            storage.trace();
            chain.txpool_trace()?;

            if run_env.chain_blocks > 0
                && block_view.number() - start_number >= run_env.chain_blocks
            {
                break;
            }

            sleep_millis(run_env.step_interval);
        }

        log::info!("Finishing work, please wait...");
        chain.txpool_save_pool()?;

        drop(chain);
        drop(storage);

        Ok(())
    }
}

fn sleep_millis(interval: u64) {
    thread::sleep(time::Duration::from_millis(interval));
}
