use std::{collections::HashMap, result::Result as StdResult};

use ckb_types::{core::TransactionView, packed, prelude::*};
use indexmap::IndexMap;

use super::Storage;
use crate::{
    error::{Error, Result},
    types::{RandomGenerator, TxOutputsStatus, TxStatus},
};

type TxUpdates = HashMap<packed::Byte32, TxStatus>;

pub(crate) struct TxOverlay {
    view: TransactionView,
    changes: TxOverlayChanges,
}

pub(crate) enum TxOverlayChanges {
    Pending {
        new: TxOutputsStatus,
        updates: TxUpdates,
    },
    Committed {
        new: TxOutputsStatus,
        updates: TxUpdates,
    },
    Failed {
        updates: TxUpdates,
    },
}

pub(crate) struct Overlay<'a> {
    storage: &'a Storage,
    pub(crate) txs: IndexMap<packed::Byte32, TxOverlay>,
}

impl TxOverlay {
    pub(crate) fn new(view: TransactionView, changes: TxOverlayChanges) -> Self {
        Self { view, changes }
    }

    pub(crate) fn is_failed(&self) -> bool {
        self.changes.is_failed()
    }

    pub(crate) fn changes(&self) -> StdResult<(TxStatus, TxUpdates), TxUpdates> {
        self.changes.to_res()
    }

    pub(crate) fn view(&self) -> &TransactionView {
        &self.view
    }

    pub(crate) fn status(&self) -> TxStatus {
        self.changes.to_status()
    }
}

impl TxOverlayChanges {
    fn is_failed(&self) -> bool {
        matches!(self, Self::Failed { .. })
    }

    fn to_res(&self) -> StdResult<(TxStatus, TxUpdates), TxUpdates> {
        match *self {
            Self::Pending {
                ref new,
                ref updates,
            } => Ok((TxStatus::Pending(new.to_owned()), updates.to_owned())),
            Self::Committed {
                ref new,
                ref updates,
            } => Ok((TxStatus::Committed(new.to_owned()), updates.to_owned())),
            Self::Failed { ref updates } => Err(updates.to_owned()),
        }
    }

    fn to_status(&self) -> TxStatus {
        match *self {
            Self::Pending {
                ref new,
                updates: _,
            } => TxStatus::Pending(new.to_owned()),
            Self::Committed {
                ref new,
                updates: _,
            } => TxStatus::Committed(new.to_owned()),
            Self::Failed { updates: _ } => TxStatus::Failed,
        }
    }
}

impl<'a> Overlay<'a> {
    pub(crate) fn new(storage: &'a Storage) -> Self {
        let txs = IndexMap::new();
        Self { storage, txs }
    }

    pub(crate) fn add_tx(&mut self, tx: TxOverlay) {
        let hash = tx.view.hash();
        let result = self.txs.insert(hash, tx);
        if result.is_some() {
            panic!("Shouldn't insert same transaction into a overlay twice.");
        }
    }

    pub(crate) fn has_tx(&mut self, tx_hash: &packed::Byte32) -> bool {
        self.txs.contains_key(tx_hash)
    }

    pub(crate) fn live_cells_count(&self) -> usize {
        let mut cnt = self.storage.live_cells_count();
        for tx in self.txs.values() {
            if !tx.is_failed() {
                cnt -= tx.view.inputs().len();
                cnt -= tx.view.outputs().len();
            }
        }
        cnt
    }

    pub(crate) fn get_tx(&self, tx_hash: &packed::Byte32) -> Option<TransactionView> {
        if let Some(tx_overlay) = self.txs.get(tx_hash) {
            Some(tx_overlay.view().to_owned())
        } else {
            self.storage.get_transaction(tx_hash).unwrap()
        }
    }

    pub(crate) fn get_tx_status(&self, tx_hash: &packed::Byte32) -> Result<TxStatus> {
        for (new_tx_hash, tx_overlay) in self.txs.iter().rev() {
            if let Ok((_, updates)) = tx_overlay.changes() {
                if let Some(tx_status) = updates.get(tx_hash) {
                    return Ok(tx_status.to_owned());
                }
            }
            if new_tx_hash == tx_hash {
                return Ok(tx_overlay.status());
            }
        }
        self.storage.get_tx_status(tx_hash)?.ok_or_else(|| {
            let errmsg = format!("failed to find tx status for {:#x}", tx_hash);
            Error::runtime(errmsg)
        })
    }

    pub(crate) fn random_tx(
        &self,
        rg: &RandomGenerator,
    ) -> Result<Option<(packed::Byte32, TxStatus)>> {
        'found: for _ in 0..30 {
            let tx_hash_start = rg.random_hash().pack();
            let (mut tx_hash, mut tx_status) = self.storage.next_tx_status(&tx_hash_start)?;
            let mut new_cell_since = None;
            for (index, (new_tx_hash, tx_overlay)) in self.txs.iter().enumerate() {
                if new_tx_hash < &tx_hash {
                    new_cell_since = Some(index);
                    tx_hash = new_tx_hash.to_owned();
                    tx_status = tx_overlay.status();
                }
            }
            let skipped = new_cell_since.map(|index| index + 1).unwrap_or(0);
            for (_, tx_overlay) in self.txs.iter().skip(skipped).rev() {
                if let Err(updates) = tx_overlay.changes() {
                    if updates.get(&tx_hash).is_some() {
                        continue 'found;
                    }
                }
            }
            for (_, tx_overlay) in self.txs.iter().skip(skipped).rev() {
                if let Ok((_, updates)) = tx_overlay.changes() {
                    if let Some(new_tx_status) = updates.get(&tx_hash) {
                        tx_status = new_tx_status.clone();
                        break;
                    }
                }
            }
            return Ok(Some((tx_hash, tx_status)));
        }
        Ok(None)
    }
}
