use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::Arc,
};

use ckb_app_config::{BlockAssemblerConfig, NetworkConfig, TxPoolConfig};
use ckb_async_runtime::{new_global_runtime, Handle};
use ckb_chain_spec::{
    build_genesis_type_id_script, calculate_block_reward,
    consensus::{build_genesis_epoch_ext, Consensus, ConsensusBuilder},
    OUTPUT_INDEX_DAO,
};
use ckb_channel::Receiver;
use ckb_dao_utils::genesis_dao_data_with_satoshi_gift;
use ckb_network::{DefaultExitHandler, NetworkController, NetworkService, NetworkState};
use ckb_pow::Pow;
use ckb_proposal_table::{ProposalTable, ProposalView};
use ckb_script::mock::MockedScripts;
use ckb_snapshot::Snapshot;
use ckb_stop_handler::StopHandler;
use ckb_store::{ChainDB, ChainStore as _};
use ckb_system_scripts::BUNDLED_CELL;
use ckb_test_chain_utils::always_success_cell;
use ckb_tx_pool::{
    service::TxVerificationResult, BlockTemplate, TokioRwLock, TxEntry, TxPool, TxPoolController,
    TxPoolServiceBuilder,
};
use ckb_types::{
    core::{
        capacity_bytes, hardfork::HardForkSwitch, tx_pool::Reject, BlockView, Capacity, DepType,
        EpochExt, EpochNumber, EpochNumberWithFraction, FeeRate, HeaderView, ScriptHashType,
        TransactionView,
    },
    packed,
    prelude::*,
};
use ckb_verification::cache::init_cache;
use ckb_verification_traits::Verifier;

use super::MockedStore;
use crate::{
    error::{Error, Result},
    types::{ChainSpec, Params, ScriptAnchor},
    utils,
};

const CONSENSUS_ID: &str = "ckb-txpool-fuzzer";
const NETWORK_NAME: &str = "CKB Mocked Network";

pub(crate) struct MockedChain {
    consensus: Arc<Consensus>,
    store: MockedStore,
    current_snapshot: Arc<Snapshot>,
    _handle: Handle,
    _stop_handler: StopHandler<()>,
    tx_pool_controller: TxPoolController,
    _network_controller: NetworkController,
    _tx_relay_receiver: Receiver<TxVerificationResult>,
    proposal_table: ProposalTable,
}

// Init
impl MockedChain {
    pub(crate) fn init<P: AsRef<Path>>(data_dir: P, cfg: &ChainSpec) -> Result<()> {
        let store_dir = data_dir.as_ref().join("chain");
        utils::fs::check_directory(&store_dir, false)?;
        let store = MockedStore::init(store_dir);

        let consensus = Arc::new(Self::build_consensus(cfg)?);
        ckb_verification::GenesisVerifier::new()
            .verify(&consensus)
            .map_err(|err| {
                let errmsg = format!("failed to verify the genesis block since {}", err);
                Error::config(errmsg)
            })?;

        store.store().init(&consensus).map_err(Error::runtime)?;
        Ok(())
    }

    fn build_consensus(cfg: &ChainSpec) -> Result<Consensus> {
        let hardfork_switch = Self::build_hardfork_switch(&cfg.params)?;
        let genesis_epoch_ext = build_genesis_epoch_ext(
            cfg.params.initial_primary_epoch_reward(),
            cfg.genesis.compact_target,
            cfg.params.genesis_epoch_length(),
            cfg.params.epoch_duration_target(),
            cfg.params.orphan_rate_target(),
        );
        let genesis_block = Self::build_genesis_block(cfg)?;
        let pow = Pow::Dummy;
        let consensus = ConsensusBuilder::new(genesis_block, genesis_epoch_ext)
            .id(CONSENSUS_ID.to_owned())
            .cellbase_maturity(EpochNumberWithFraction::from_full_value(
                cfg.params.cellbase_maturity(),
            ))
            .secondary_epoch_reward(cfg.params.secondary_epoch_reward())
            .max_block_cycles(cfg.params.max_block_cycles())
            .max_block_bytes(cfg.params.max_block_bytes())
            .pow(pow)
            .primary_epoch_reward_halving_interval(
                cfg.params.primary_epoch_reward_halving_interval(),
            )
            .initial_primary_epoch_reward(cfg.params.initial_primary_epoch_reward())
            .epoch_duration_target(cfg.params.epoch_duration_target())
            .permanent_difficulty_in_dummy(cfg.params.permanent_difficulty_in_dummy())
            .max_block_proposals_limit(cfg.params.max_block_proposals_limit())
            .orphan_rate_target(cfg.params.orphan_rate_target())
            .hardfork_switch(hardfork_switch)
            .build();
        Ok(consensus)
    }

    fn build_hardfork_switch(cfg: &Params) -> Result<HardForkSwitch> {
        cfg.hardfork
            .as_ref()
            .cloned()
            .unwrap_or_default()
            .complete_with_default(EpochNumber::MAX)
            .map_err(Error::config)
    }

    // Transactions in Genesis Block:
    // - tx0: Cellbase.
    //   - Deploy always success script.
    //   - Burned cell.
    //   - Input cell for tx1.
    // - tx1: Deploy always success script again with type script.
    fn build_genesis_block(cfg: &ChainSpec) -> Result<BlockView> {
        let (_, script_data, _) = always_success_cell();
        let script_data_capacity = Capacity::bytes(script_data.len()).unwrap();
        let script_data_hash = packed::CellOutput::calc_data_hash(script_data);
        let tmp_consensus = ConsensusBuilder::default().build();

        let input = packed::CellInput::new_cellbase_input(0);

        let script_as_data_hash_type = packed::Script::new_builder()
            .hash_type(ScriptHashType::Data.into())
            .code_hash(script_data_hash)
            .build();

        let output_tx1 = packed::CellOutput::new_builder()
            .type_(Some(script_as_data_hash_type.clone()).pack())
            .build_exact_capacity(script_data_capacity)
            .unwrap();

        let cellbase = {
            let output_deploy_script = packed::CellOutput::new_builder()
                .build_exact_capacity(script_data_capacity)
                .unwrap();
            let output_as_tx1_input = packed::CellOutput::new_builder()
                .lock(script_as_data_hash_type.clone())
                .capacity(output_tx1.capacity())
                .build();
            let output_data_dao = BUNDLED_CELL.get("specs/cells/dao").unwrap().into_owned();
            let output_dao = {
                let output_data_dao_capacity = Capacity::bytes(output_data_dao.len()).unwrap();
                packed::CellOutput::new_builder()
                    .capacity(capacity_bytes!(16_000).pack())
                    .type_(Some(build_genesis_type_id_script(OUTPUT_INDEX_DAO)).pack())
                    .build_exact_capacity(output_data_dao_capacity)
                    .unwrap()
            };
            let output_burned = {
                let script_burned = packed::Script::new_builder()
                    .hash_type(ScriptHashType::Data.into())
                    .args(tmp_consensus.satoshi_pubkey_hash.as_bytes().pack())
                    .build();
                packed::CellOutput::new_builder()
                    .capacity(capacity_bytes!(8_400_000_000).pack())
                    .lock(script_burned)
                    .build()
            };

            TransactionView::new_advanced_builder()
                .input(input)
                // Cell 0: always success script
                .output(output_deploy_script)
                .output_data(script_data.pack())
                // Cell 1: cell as tx1 input
                .output(output_as_tx1_input)
                .output_data(Default::default())
                // Cell 2: dao
                // Ref: `ckb-chain-spec::OUTPUT_INDEX_DAO`
                .output(output_dao)
                .output_data(output_data_dao.pack())
                // Cell 3: burned
                .output(output_burned)
                .output_data(Default::default())
                .witness(script_as_data_hash_type.clone().into_witness())
                .build()
        };

        let script_hash = script_as_data_hash_type.calc_script_hash();

        let tx1 = {
            let script_as_data_type_cell_dep = {
                let script_as_data_type_op = packed::OutPoint::new(cellbase.hash(), 0);
                packed::CellDep::new_builder()
                    .out_point(script_as_data_type_op)
                    .dep_type(DepType::Code.into())
                    .build()
            };
            let script_as_type_hash_type = packed::Script::new_builder()
                .code_hash(script_hash)
                .hash_type(ScriptHashType::Type.into())
                .build();
            let input_op = packed::OutPoint::new(cellbase.hash(), 1);
            let input = packed::CellInput::new(input_op, 0);
            TransactionView::new_advanced_builder()
                .cell_dep(script_as_data_type_cell_dep)
                .input(input)
                .output(output_tx1)
                .output_data(script_data.pack())
                .witness(script_as_type_hash_type.into_witness())
                .build()
        };

        let dao = {
            let epoch_length = cfg.params.genesis_epoch_length();
            let primary_issuance =
                calculate_block_reward(cfg.params.initial_primary_epoch_reward(), epoch_length);
            let secondary_issuance =
                calculate_block_reward(cfg.params.secondary_epoch_reward(), epoch_length);
            genesis_dao_data_with_satoshi_gift(
                vec![&cellbase, &tx1],
                &tmp_consensus.satoshi_pubkey_hash,
                tmp_consensus.satoshi_cell_occupied_ratio,
                primary_issuance,
                secondary_issuance,
            )
            .unwrap()
        };
        let genesis_block = packed::Block::new_advanced_builder()
            .timestamp(cfg.genesis.timestamp.pack())
            .dao(dao)
            .compact_target(cfg.genesis.compact_target.pack())
            .transaction(cellbase)
            .transaction(tx1)
            .build();
        Ok(genesis_block)
    }
}

// Load
impl MockedChain {
    pub(crate) fn load<P: AsRef<Path>>(data_dir: P, cfg: &ChainSpec) -> Result<Self> {
        let store_dir = data_dir.as_ref().join("chain");
        utils::fs::check_directory(&store_dir, true)?;
        let store = MockedStore::init(store_dir);

        let consensus = Arc::new(Self::build_consensus(cfg)?);

        let (current_snapshot, proposal_table) =
            Self::initialize_current_snapshot(&consensus, &store);
        let (handle, stop_handler) = new_global_runtime();
        let network_dir = data_dir.as_ref().join("network");
        let network_controller = Self::dummy_network(network_dir, &handle)?;
        let tx_pool_dir = data_dir.as_ref().join("tx_pool");
        utils::fs::need_directory(&tx_pool_dir)?;
        let always_sucess = Self::always_sucess_from_genesis_block(consensus.genesis_block());
        MockedScripts::insert_data_hash(always_sucess.data_hash());
        MockedScripts::insert_type_hash(always_sucess.type_hash());
        let (tx_pool_controller, tx_relay_receiver) = Self::build_tx_pool(
            tx_pool_dir,
            &handle,
            &current_snapshot,
            &network_controller,
            &always_sucess,
        )?;

        Ok(Self {
            consensus,
            store,
            current_snapshot,
            _handle: handle,
            _stop_handler: stop_handler,
            tx_pool_controller,
            _network_controller: network_controller,
            _tx_relay_receiver: tx_relay_receiver,
            proposal_table,
        })
    }

    fn initialize_current_snapshot(
        consensus: &Arc<Consensus>,
        store: &MockedStore,
    ) -> (Arc<Snapshot>, ProposalTable) {
        let (proposal_table, proposals) = Self::init_proposal_table(consensus, store);
        let store = store.store().get_snapshot();
        let tip_header = store.get_tip_header().unwrap();
        let tip_hash = tip_header.hash();
        let total_difficulty = store
            .get_block_ext(&tip_hash)
            .map(|block_ext| block_ext.total_difficulty)
            .unwrap();
        let current_epoch_ext = store
            .get_block_epoch_index(&tip_hash)
            .and_then(|index| store.get_epoch_ext(&index))
            .unwrap();
        let snapshot = Snapshot::new(
            tip_header,
            total_difficulty,
            current_epoch_ext,
            store,
            proposals,
            Arc::clone(consensus),
        );
        (Arc::new(snapshot), proposal_table)
    }

    fn dummy_network(network_dir: PathBuf, handle: &Handle) -> Result<NetworkController> {
        let exit_handler = DefaultExitHandler::default();
        let config = NetworkConfig {
            max_peers: 20,
            max_outbound_peers: 5,
            path: network_dir,
            ping_interval_secs: 15,
            ping_timeout_secs: 20,
            connect_outbound_interval_secs: 1,
            discovery_local_address: true,
            bootnode_mode: true,
            reuse_port_on_linux: true,
            ..Default::default()
        };
        let network_state = Arc::new(NetworkState::from_config(config).unwrap());
        NetworkService::new(
            network_state,
            vec![],
            vec![],
            NETWORK_NAME.to_owned(),
            clap::crate_version!().to_owned(),
            exit_handler,
        )
        .start(handle)
        .map_err(|err| {
            let errmsg = format!("failed to start network since {}", err);
            Error::runtime(errmsg)
        })
    }

    fn build_tx_pool(
        tx_pool_dir: PathBuf,
        handle: &Handle,
        current_snapshot: &Arc<Snapshot>,
        network_controller: &NetworkController,
        always_sucess: &ScriptAnchor,
    ) -> Result<(TxPoolController, Receiver<TxVerificationResult>)> {
        let tx_pool_config = TxPoolConfig {
            min_fee_rate: FeeRate(0),
            persisted_data: tx_pool_dir.join("persisted_data"),
            ..Default::default()
        };
        let args = {
            let mut tmp = vec![0u8; 32];
            let result_bytes = 0u64.to_le_bytes();
            let cycles_bytes = 500u64.to_le_bytes();
            (&mut tmp[0..8]).copy_from_slice(&result_bytes);
            (&mut tmp[8..16]).copy_from_slice(&cycles_bytes);
            (&mut tmp[16..24]).copy_from_slice(&result_bytes);
            (&mut tmp[24..32]).copy_from_slice(&cycles_bytes);
            tmp
        };
        let block_assembler_config = BlockAssemblerConfig {
            code_hash: always_sucess.type_hash().unpack(),
            args: args.pack().into(),
            hash_type: ScriptHashType::Type.into(),
            message: Default::default(),
            use_binary_version_as_message_prefix: false,
            binary_version: clap::crate_version!().to_owned(),
        };
        let txs_verify_cache = {
            let cache = init_cache();
            Arc::new(TokioRwLock::new(cache))
        };
        let (tx_relay_sender, tx_relay_receiver) = ckb_channel::unbounded();
        let (mut tx_pool_builder, tx_pool_controller) = TxPoolServiceBuilder::new(
            tx_pool_config,
            Arc::clone(current_snapshot),
            Some(block_assembler_config),
            txs_verify_cache,
            handle,
            tx_relay_sender,
        );
        Self::register_tx_pool_callback(&mut tx_pool_builder);
        tx_pool_builder.start(network_controller.clone());
        if tx_pool_controller.service_started() {
            Ok((tx_pool_controller, tx_relay_receiver))
        } else {
            Err(Error::runtime("failed to start tx-pool"))
        }
    }
}

// Copy from CKB.
impl MockedChain {
    // Copy from ckb/util/launcher/src/shared_builder.rs
    fn init_proposal_table(
        consensus: &Arc<Consensus>,
        store: &MockedStore,
    ) -> (ProposalTable, ProposalView) {
        let store = store.store().get_snapshot();
        let proposal_window = consensus.tx_proposal_window();
        let tip_number = store.get_tip_header().unwrap().number();
        let mut proposal_ids = ProposalTable::new(proposal_window);
        let proposal_start = tip_number.saturating_sub(proposal_window.farthest());
        for bn in proposal_start..=tip_number {
            if let Some(hash) = store.get_block_hash(bn) {
                let mut ids_set = HashSet::new();
                if let Some(ids) = store.get_block_proposal_txs_ids(&hash) {
                    ids_set.extend(ids)
                }

                if let Some(us) = store.get_block_uncles(&hash) {
                    for u in us.data().into_iter() {
                        ids_set.extend(u.proposals().into_iter());
                    }
                }
                proposal_ids.insert(bn, ids_set);
            }
        }
        let dummy_proposals = ProposalView::default();
        let (_, proposals) = proposal_ids.finalize(&dummy_proposals, tip_number);
        (proposal_ids, proposals)
    }

    // Copy from ckb/util/launcher/src/shared_builder.rs
    fn register_tx_pool_callback(tx_pool_builder: &mut TxPoolServiceBuilder) {
        tx_pool_builder.register_pending(Box::new(move |tx_pool: &mut TxPool, entry: &TxEntry| {
            tx_pool.update_statics_for_add_tx(entry.size, entry.cycles);
        }));

        tx_pool_builder.register_proposed(Box::new(
            move |tx_pool: &mut TxPool, entry: &TxEntry, new: bool| {
                if new {
                    tx_pool.update_statics_for_add_tx(entry.size, entry.cycles);
                }
            },
        ));

        tx_pool_builder.register_committed(Box::new(
            move |tx_pool: &mut TxPool, entry: &TxEntry| {
                tx_pool.update_statics_for_remove_tx(entry.size, entry.cycles);
            },
        ));

        tx_pool_builder.register_reject(Box::new(
            move |tx_pool: &mut TxPool, entry: &TxEntry, reject: Reject| {
                tx_pool.update_statics_for_remove_tx(entry.size, entry.cycles);
                let tx_hash = entry.transaction().hash();
                if matches!(reject, Reject::Resolve(..)) {
                    if let Some(ref mut recent_reject) = tx_pool.recent_reject {
                        let _ = recent_reject.put(&tx_hash, reject);
                    }
                }
            },
        ));
    }
}

impl MockedChain {
    pub(crate) fn store(&self) -> &ChainDB {
        self.store.store()
    }

    pub(crate) fn mocked_script(&self) -> ScriptAnchor {
        let genesis_block = self.consensus.genesis_block();
        Self::always_sucess_from_genesis_block(genesis_block)
    }

    fn always_sucess_from_genesis_block(genesis_block: &BlockView) -> ScriptAnchor {
        let tx1 = genesis_block.transaction(1).unwrap();
        let index: usize = 0;
        let cell_dep = {
            let out_point = packed::OutPoint::new(tx1.hash(), index as u32);
            packed::CellDep::new_builder()
                .out_point(out_point)
                .dep_type(DepType::Code.into())
                .build()
        };
        let data_hash = tx1
            .outputs_data()
            .get(index)
            .map(|data| packed::CellOutput::calc_data_hash(data.as_slice()))
            .unwrap();
        let type_hash = tx1
            .output(index)
            .and_then(|output| output.type_().to_opt())
            .map(|script| script.calc_script_hash())
            .unwrap();
        ScriptAnchor::new(cell_dep, data_hash, type_hash)
    }

    fn current_snapshot(&self) -> Arc<Snapshot> {
        Arc::clone(&self.current_snapshot)
    }

    fn tx_pool_controller(&self) -> &TxPoolController {
        &self.tx_pool_controller
    }

    pub(crate) fn next_epoch_ext(&self) -> EpochExt {
        self.consensus
            .next_epoch_ext(
                self.current_snapshot().tip_header(),
                &self.store.store().as_data_provider(),
            )
            .unwrap()
            .epoch()
    }
}

// Chain
impl MockedChain {
    pub(crate) fn chain_tip_header(&self) -> HeaderView {
        self.current_snapshot().tip_header().to_owned()
    }

    pub(crate) fn chain_submit_block(&mut self, block: &BlockView) {
        let next_epoch_ext = self.next_epoch_ext();
        self.store.insert_block(block, &next_epoch_ext);
        self.store.attach_block(&block.hash());
        self.store.set_block_as_tip(&block.hash());
        let (current_snapshot, proposal_table) =
            Self::initialize_current_snapshot(&self.consensus, &self.store);
        self.current_snapshot = current_snapshot;
        self.proposal_table = proposal_table;
    }
}

// TxPool
impl MockedChain {
    pub(crate) fn txpool_trace(&self) -> Result<()> {
        let info = self
            .tx_pool_controller()
            .get_tx_pool_info()
            .map_err(Error::runtime)?;
        log::trace!(
            "[TxPool] tip: {}, hash: {:#x}, ts: {}, \
            pending: {}, proposed: {}, orphan: {}, \
            total_size: {}, total_cycles: {}",
            info.tip_number,
            info.tip_hash,
            info.last_txs_updated_at,
            info.pending_size,
            info.proposed_size,
            info.orphan_size,
            info.total_tx_size,
            info.total_tx_cycles,
        );
        Ok(())
    }

    pub(crate) fn txpool_save_pool(&self) -> Result<()> {
        self.tx_pool_controller()
            .save_pool()
            .map_err(Error::runtime)
    }

    pub(crate) fn get_block_template(&self) -> Result<BlockTemplate> {
        let snapshot = self.current_snapshot();
        self.tx_pool_controller()
            .get_block_template(None, None, None, snapshot)
            .map_err(Error::runtime)?
            .map_err(Error::runtime)
    }

    pub(crate) fn txpool_submit_block(&self, block: &BlockView) -> Result<()> {
        let snapshot = self.current_snapshot();
        let detached_blocks = VecDeque::default();
        let attached_blocks = vec![block.to_owned()].into_iter().collect();
        let detached_proposal_id = HashSet::default();
        self.tx_pool_controller()
            .update_tx_pool_for_reorg(
                detached_blocks,
                attached_blocks,
                detached_proposal_id,
                snapshot,
            )
            .map_err(Error::runtime)
    }

    pub(crate) fn txpool_submit_local_tx(&self, tx: &TransactionView) -> Result<()> {
        self.tx_pool_controller()
            .submit_local_tx(tx.clone())
            .map_err(Error::runtime)?
            .map_err(Error::runtime)
    }
}
