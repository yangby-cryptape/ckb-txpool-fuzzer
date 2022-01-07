use std::{cell::RefCell, collections::HashMap, path::Path, str::FromStr};

use ckb_types::{
    core::{BlockView, TransactionView},
    packed,
    prelude::*,
};
use rocksdb::ops::{
    DeleteCF as _, Get as _, GetCF as _, GetColumnFamilys as _, IterateCF as _, OpenCF as _,
    Put as _, PutCF as _,
};

use crate::{
    error::{Error, Result},
    types::{CacheStats, MetaData, TxStatus},
    utils,
};

const KEY_METADATA: &[u8] = b"meta_data";

pub(crate) struct Storage {
    db: rocksdb::DB,
    stats: RefCell<CacheStats>,
}

// Construction
impl Storage {
    // Only store those blocks which are not in main chain.
    const CF_BLOCKS: &'static str = "blocks";
    // Only store those transactions which are not in main chain.
    const CF_TXS: &'static str = "transactions";

    // Store outputs statuses for all available transactions.
    const CF_TX_STATUSES: &'static str = "tx_statuses";
    // Store all transactions which are invalid but haven't been committed.
    const CF_PENDING_TXS: &'static str = "pending_txs";

    const CF_NAMES: &'static [&'static str] = &[
        Self::CF_BLOCKS,
        Self::CF_TXS,
        Self::CF_TX_STATUSES,
        Self::CF_PENDING_TXS,
    ];

    pub(crate) fn init<P: AsRef<Path>>(path: P) -> Result<Self> {
        let db = Self::open(path, true)?;
        let stats = RefCell::new(CacheStats::default());
        let ret = Self { db, stats };
        Ok(ret)
    }

    pub(crate) fn load<P: AsRef<Path>>(path: P) -> Result<Self> {
        let db = Self::open(path, false)?;
        let stats = RefCell::new(CacheStats::default());
        let ret = Self { db, stats };
        ret.load_tx_statuses()?;
        Ok(ret)
    }

    fn open<P: AsRef<Path>>(path: P, create: bool) -> Result<rocksdb::DB> {
        utils::fs::check_directory(&path, !create)?;
        let opts = Self::default_dboptions(create);
        let cfs = Self::default_column_family_descriptors();
        let db = rocksdb::DB::open_cf_descriptors(&opts, &path, cfs)?;
        Ok(db)
    }

    fn default_dboptions(create: bool) -> rocksdb::Options {
        let mut opts = rocksdb::Options::default();
        if create {
            opts.create_if_missing(true);
            opts.create_missing_column_families(true);
        } else {
            opts.create_if_missing(false);
            opts.create_missing_column_families(false);
        }
        // DBOptions
        opts.set_bytes_per_sync(1 << 20);
        // TODO RocksDB API
        opts.set_max_background_compactions(2);
        opts.set_max_background_flushes(2);
        // opts.set_max_background_jobs(4);
        opts.set_max_total_wal_size((1 << 20) * 64);
        opts.set_keep_log_file_num(64);
        opts.set_max_open_files(64);
        // CFOptions "default"
        opts.set_level_compaction_dynamic_level_bytes(true);
        opts.set_write_buffer_size((1 << 20) * 8);
        opts.set_min_write_buffer_number_to_merge(1);
        opts.set_max_write_buffer_number(2);
        // TODO RocksDB API
        // opts.set_max_write_buffer_size_to_maintain(-1);
        // [TableOptions/BlockBasedTable "default"]
        let block_opts = {
            let mut block_opts = rocksdb::BlockBasedOptions::default();
            block_opts.set_cache_index_and_filter_blocks(true);
            block_opts.set_pin_l0_filter_and_index_blocks_in_cache(true);
            block_opts
        };

        opts.set_block_based_table_factory(&block_opts);

        opts
    }

    fn default_cfoptions() -> rocksdb::Options {
        let mut opts = rocksdb::Options::default();
        opts.set_level_compaction_dynamic_level_bytes(true);
        opts.set_write_buffer_size((1 << 20) * 8);
        opts.set_min_write_buffer_number_to_merge(1);
        opts.set_max_write_buffer_number(2);
        // TODO RocksDB API
        // opts.set_max_write_buffer_size_to_maintain(-1);
        opts
    }

    fn default_column_family_descriptors() -> Vec<rocksdb::ColumnFamilyDescriptor> {
        let cfopts = Self::default_cfoptions();
        Self::CF_NAMES
            .iter()
            .map(|name| rocksdb::ColumnFamilyDescriptor::new(name.to_owned(), cfopts.clone()))
            .collect()
    }
}

// Common
impl Storage {
    fn cf_handle(&self, cf_name: &str) -> Result<&rocksdb::ColumnFamily> {
        self.db.cf_handle(cf_name).ok_or_else(|| {
            let errmsg = format!("column family {} should exists", cf_name);
            Error::storage(errmsg)
        })
    }

    pub(crate) fn trace(&self) {
        log::trace!("[Storage] stats: {}", self.stats.borrow());
    }
}

// CF: Default
impl Storage {
    pub(crate) fn put_meta_data(&self, meta_data: &MetaData) -> Result<()> {
        self.db
            .put(KEY_METADATA, meta_data.to_string().as_bytes())
            .map_err(Into::into)
    }

    pub(crate) fn get_meta_data(&self) -> Result<MetaData> {
        self.db
            .get(KEY_METADATA)
            .map_err::<Error, _>(Into::into)?
            .map(|slice| String::from_utf8(slice.to_vec()).map_err(Error::storage))
            .transpose()?
            .map(|s| FromStr::from_str(&s).map_err(Error::storage))
            .transpose()?
            .ok_or_else(|| Error::storage("can not found the meta_data"))
    }
}

// CF: Transactions
impl Storage {
    fn put_transaction(&self, tx: &TransactionView) -> Result<()> {
        let cf = self.cf_handle(Self::CF_TXS)?;
        let hash = tx.hash();
        self.db
            .put_cf(cf, hash.as_slice(), tx.data().as_slice())
            .map_err(Into::into)
    }

    pub(crate) fn get_transaction(
        &self,
        tx_hash: &packed::Byte32,
    ) -> Result<Option<TransactionView>> {
        let cf = self.cf_handle(Self::CF_TXS)?;
        self.db
            .get_cf(cf, tx_hash.as_slice())?
            .map(|tx| {
                packed::Transaction::from_slice(&tx)
                    .map(packed::Transaction::into_view)
                    .map_err(Error::storage)
            })
            .transpose()
    }

    fn delete_transaction(&self, tx_hash: &packed::Byte32) -> Result<()> {
        let cf = self.cf_handle(Self::CF_TXS)?;
        self.db
            .delete_cf(cf, tx_hash.as_slice())
            .map_err(Into::into)
    }
}

// CF: TXs' statuses
impl Storage {
    fn put_tx_status(&self, tx_hash: packed::Byte32, tx_status: TxStatus) -> Result<()> {
        let cf = self.cf_handle(Self::CF_TX_STATUSES)?;
        self.db
            .put_cf(cf, tx_hash.as_slice(), tx_status.to_vec()?)?;
        Ok(())
    }

    pub(crate) fn get_tx_status(&self, tx_hash: &packed::Byte32) -> Result<Option<TxStatus>> {
        let cf = self.cf_handle(Self::CF_TX_STATUSES)?;
        self.db
            .get_cf(cf, tx_hash.as_slice())?
            .map(|tx| TxStatus::from_slice(&tx).map_err(Error::storage))
            .transpose()
    }

    fn delete_tx_status(&self, tx_hash: &packed::Byte32) -> Result<()> {
        let cf = self.cf_handle(Self::CF_TX_STATUSES)?;
        self.db
            .delete_cf(cf, tx_hash.as_slice())
            .map_err(Into::into)
    }

    pub(crate) fn next_tx_status(
        &self,
        tx_hash: &packed::Byte32,
    ) -> Result<(packed::Byte32, TxStatus)> {
        let cf = self.cf_handle(Self::CF_TX_STATUSES)?;
        let mode = rocksdb::IteratorMode::From(tx_hash.as_slice(), rocksdb::Direction::Forward);
        let next = self
            .db
            .full_iterator_cf(cf, mode)?
            .next()
            .ok_or_else(|| {
                let errmsg = format!("no available cells from {:#x}", tx_hash);
                Error::storage(errmsg)
            })
            .and_then(|(key, value)| {
                let tx_hash = packed::Byte32::from_slice(&key).map_err(Error::storage)?;
                let tx_status = TxStatus::from_slice(&value).map_err(Error::storage)?;
                Ok((tx_hash, tx_status))
            });
        if next.is_ok() {
            return next;
        }
        self.db
            .full_iterator_cf(cf, rocksdb::IteratorMode::Start)?
            .next()
            .ok_or_else(|| Error::storage("no available cells from start"))
            .and_then(|(key, value)| {
                let tx_hash = packed::Byte32::from_slice(&key).map_err(Error::storage)?;
                let tx_status = TxStatus::from_slice(&value).map_err(Error::storage)?;
                Ok((tx_hash, tx_status))
            })
    }

    fn load_tx_statuses(&self) -> Result<()> {
        let cf = self.cf_handle(Self::CF_TX_STATUSES)?;
        for (_, value) in self.db.full_iterator_cf(cf, rocksdb::IteratorMode::Start)? {
            let tx_status = TxStatus::from_slice(&value).map_err(Error::storage)?;
            self.stats.borrow_mut().load_tx(&tx_status);
        }
        Ok(())
    }

    pub(crate) fn live_cells_count(&self) -> usize {
        self.stats.borrow().cell_live_cnt()
    }
}

// CF: Pending transactions not in TXs' statuses
impl Storage {
    fn put_pending_tx(&self, tx_hash: packed::Byte32) -> Result<()> {
        let cf = self.cf_handle(Self::CF_PENDING_TXS)?;
        self.db.put_cf(cf, tx_hash.as_slice(), &[])?;
        Ok(())
    }

    fn has_pending_tx(&self, tx_hash: &packed::Byte32) -> Result<bool> {
        let cf = self.cf_handle(Self::CF_PENDING_TXS)?;
        let had = self.db.get_cf(cf, tx_hash.as_slice())?.is_some();
        Ok(had)
    }

    fn delete_pending_tx(&self, tx_hash: &packed::Byte32) -> Result<()> {
        let cf = self.cf_handle(Self::CF_PENDING_TXS)?;
        self.db
            .delete_cf(cf, tx_hash.as_slice())
            .map_err(Into::into)
    }
}

// Hybrid
impl Storage {
    pub(crate) fn submit_tx(
        &self,
        tx: &TransactionView,
        tx_status: TxStatus,
        changes: HashMap<packed::Byte32, TxStatus>,
    ) -> Result<()> {
        let inputs_count = tx.inputs().len();
        self.stats
            .borrow_mut()
            .submit_tx(inputs_count, &tx_status)?;
        self.put_transaction(tx)?;
        self.put_tx_status(tx.hash(), tx_status)?;
        for (hash, status) in changes {
            self.put_tx_status(hash, status)?;
        }
        Ok(())
    }

    pub(crate) fn submit_invalid_tx(&self, tx: &TransactionView) -> Result<()> {
        let tx_status = TxStatus::Failed;
        self.stats.borrow_mut().submit_tx(0, &tx_status)?;
        self.put_transaction(tx)?;
        self.put_tx_status(tx.hash(), tx_status)?;
        Ok(())
    }

    pub(crate) fn remove_invalid_tx(
        &self,
        tx_hash: &packed::Byte32,
        tx_status: &TxStatus,
    ) -> Result<()> {
        if matches!(tx_status, TxStatus::Pending(_)) {
            self.put_pending_tx(tx_hash.to_owned())?;
        }
        self.delete_transaction(tx_hash)?;
        self.delete_tx_status(tx_hash)?;
        self.stats.borrow_mut().remove_tx(tx_status);
        Ok(())
    }

    pub(crate) fn confirm_block(&self, block: &BlockView) -> Result<()> {
        let cf_blocks = self.cf_handle(Self::CF_BLOCKS)?;
        self.db.delete_cf(cf_blocks, block.hash().as_slice())?;
        let mut is_cellbase = true;
        for tx in block.transactions() {
            let tx_hash = tx.hash();
            if is_cellbase {
                if !tx.outputs().is_empty() {
                    log::trace!("[Storage] commit cellbase {:#x}", tx_hash);
                    let outputs_count = tx.outputs().len();
                    let tx_status = TxStatus::new_committed(outputs_count);
                    self.put_tx_status(tx_hash, tx_status)?;
                    self.stats.borrow_mut().commit_cellbase(outputs_count);
                }
                is_cellbase = false;
            } else {
                self.delete_transaction(&tx_hash)?;
                if let Some(tx_status) = self.get_tx_status(&tx_hash)? {
                    match tx_status {
                        TxStatus::Failed => {
                            let errmsg =
                                format!("tx {:#x} is committed but it should be failed", tx_hash);
                            return Err(Error::runtime(errmsg));
                        }
                        TxStatus::Committed(..) => {
                            let errmsg =
                                format!("tx {:#x} is committed but it already committed", tx_hash);
                            return Err(Error::runtime(errmsg));
                        }
                        TxStatus::Pending(inner) => {
                            log::trace!("[Storage] commit pending {:#x}", tx_hash);
                            let new_tx_status = TxStatus::Committed(inner);
                            self.put_tx_status(tx_hash, new_tx_status)?;
                            self.stats.borrow_mut().commit_pending();
                        }
                    }
                } else if self.has_pending_tx(&tx_hash)? {
                    self.delete_pending_tx(&tx_hash)?;
                } else {
                    let errmsg = format!("tx {:#x} is committed but it's unknown", tx_hash);
                    return Err(Error::runtime(errmsg));
                }
            }
        }
        Ok(())
    }
}
