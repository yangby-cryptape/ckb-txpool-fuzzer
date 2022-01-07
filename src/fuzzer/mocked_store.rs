use std::{path::Path, sync::Arc};

use ckb_db::RocksDB;
use ckb_db_schema::COLUMNS;
use ckb_store::{attach_block_cell, ChainDB, ChainStore};
use ckb_types::{
    core::{
        cell::{CellMetaBuilder, CellProvider, CellStatus, HeaderChecker},
        error::OutPointError,
        BlockExt, BlockView, EpochExt,
    },
    packed,
    prelude::*,
};
use faketime::unix_time_as_millis;

#[derive(Clone)]
pub(crate) struct MockedStore {
    inner: Arc<ChainDB>,
}

impl MockedStore {
    pub(crate) fn init<P: AsRef<Path>>(store_dir: P) -> Self {
        let db = RocksDB::open_in(&store_dir, COLUMNS);
        Self {
            inner: Arc::new(ChainDB::new(db, Default::default())),
        }
    }

    pub(crate) fn store(&self) -> &ChainDB {
        &self.inner
    }

    pub(crate) fn insert_block(&self, block: &BlockView, epoch_ext: &EpochExt) {
        let db_txn = self.store().begin_transaction();
        let last_block_hash_in_previous_epoch = epoch_ext.last_block_hash_in_previous_epoch();
        db_txn.insert_block(block).unwrap();
        {
            let parent_block_ext = self.store().get_block_ext(&block.parent_hash()).unwrap();
            let block_ext = BlockExt {
                received_at: unix_time_as_millis(),
                total_difficulty: parent_block_ext.total_difficulty.to_owned()
                    + block.header().difficulty(),
                total_uncles_count: parent_block_ext.total_uncles_count
                    + block.data().uncles().len() as u64,
                verified: Some(true),
                txs_fees: vec![],
            };
            db_txn.insert_block_ext(&block.hash(), &block_ext).unwrap();
        }
        db_txn
            .insert_block_epoch_index(&block.hash(), &last_block_hash_in_previous_epoch)
            .unwrap();
        db_txn
            .insert_epoch_ext(&last_block_hash_in_previous_epoch, epoch_ext)
            .unwrap();
        db_txn.commit().unwrap();
    }

    pub(crate) fn set_block_as_tip(&self, block_hash: &packed::Byte32) {
        let store = self.store();
        let block_header = store.get_block_header(block_hash).unwrap();
        let index = store.get_block_epoch_index(block_hash).unwrap();
        let epoch_ext = store.get_epoch_ext(&index).unwrap();
        let db_txn = store.begin_transaction();
        db_txn.insert_tip_header(&block_header).unwrap();
        db_txn.insert_current_epoch_ext(&epoch_ext).unwrap();
        db_txn.commit().unwrap();
    }

    pub(crate) fn attach_block(&self, block_hash: &packed::Byte32) {
        let store = self.store();
        let block = store.get_block(block_hash).unwrap();
        let db_txn = store.begin_transaction();
        db_txn.attach_block(&block).unwrap();
        attach_block_cell(&db_txn, &block).unwrap();
        db_txn.commit().unwrap();
    }

    /* TODO dead code
    pub(crate) fn detach_block(&self, block: &BlockView) {
        let db_txn = self.store().begin_transaction();
        db_txn.detach_block(&block).unwrap();
        db_txn.commit().unwrap();
    }

    pub(crate) fn delete_block(&self, block: &BlockView) {
        let db_txn = self.store().begin_transaction();
        db_txn.delete_block(&block).unwrap();
        db_txn.commit().unwrap();
    }
    */
}

impl CellProvider for MockedStore {
    fn cell(&self, out_point: &packed::OutPoint, _eager_load: bool) -> CellStatus {
        match self.store().get_transaction(&out_point.tx_hash()) {
            Some((tx, _)) => tx
                .outputs()
                .get(out_point.index().unpack())
                .map(|cell| {
                    let data = tx
                        .outputs_data()
                        .get(out_point.index().unpack())
                        .expect("output data");
                    let cell_meta = CellMetaBuilder::from_cell_output(cell, data.unpack())
                        .out_point(out_point.to_owned())
                        .build();

                    CellStatus::live_cell(cell_meta)
                })
                .unwrap_or(CellStatus::Unknown),
            None => CellStatus::Unknown,
        }
    }
}

impl HeaderChecker for MockedStore {
    fn check_valid(&self, block_hash: &packed::Byte32) -> Result<(), OutPointError> {
        if self.store().get_block_number(block_hash).is_some() {
            Ok(())
        } else {
            Err(OutPointError::InvalidHeader(block_hash.clone()))
        }
    }
}
