use std::{fmt, io, result::Result as StdResult};

use crate::error::{Error, Result};

#[derive(Debug, Clone)]
pub(crate) enum TxStatus {
    // The transaction will be committed in chain but it doesn't now.
    Pending(TxOutputsStatus),
    // The transaction is committed in chain.
    Committed(TxOutputsStatus),
    // The transaction couldn't be committed in chain.
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CellStatus {
    // The cell can be used as an input cell.
    Live,
    // The cell couldn't be unlocked.
    Burn,
    // The cell is already spent.
    Dead,
}

#[derive(Debug, Clone)]
pub(crate) struct TxOutputsStatus {
    // The statuses of output cells.
    // If A cell is spent, then its status is `false` (0), otherwise its status is `true` (1).
    pub(crate) statuses: Vec<CellStatus>,
}

#[derive(Default, Debug, Clone)]
pub(crate) struct CacheStats {
    tx_pending_cnt: usize,
    tx_committed_cnt: usize,
    tx_failed_cnt: usize,
    cell_live_cnt: usize,
}

impl TxStatus {
    pub(crate) fn new_committed(cells_count: usize) -> Self {
        Self::Committed(TxOutputsStatus::new_all_live(cells_count))
    }

    pub(crate) fn is_invalid(&self) -> bool {
        match self {
            Self::Pending(ref inner) | Self::Committed(ref inner) => inner.is_invalid(),
            Self::Failed => true,
        }
    }

    pub(crate) fn spent(&mut self, cell_index: usize) {
        match self {
            Self::Pending(ref mut inner) | Self::Committed(ref mut inner) => {
                inner.spent(cell_index);
            }
            Self::Failed => {
                panic!("the cell should be in an existed transaction before spent");
            }
        }
    }

    pub(crate) fn from_slice(slice: &[u8]) -> Result<Self> {
        if slice.is_empty() {
            return Err(Error::broken_since("TxStatus", "no enough data"));
        }
        let ret = match slice[0] {
            0x00 => Self::Pending(TxOutputsStatus::from_slice(&slice[1..])?),
            0x01 => Self::Committed(TxOutputsStatus::from_slice(&slice[1..])?),
            0xff => Self::Failed,
            x => {
                let errmsg = format!("transaction status type is unknown [{}]", x);
                return Err(Error::broken_since("TxStatus", &errmsg));
            }
        };
        Ok(ret)
    }

    pub(crate) fn to_vec(&self) -> Result<Vec<u8>> {
        let mut bytes = Vec::new();
        self.write_into(&mut bytes)
            .map(|_| bytes)
            .map_err(Error::runtime)
    }

    fn write_into<W: io::Write>(&self, output: &mut W) -> StdResult<(), io::Error> {
        match self {
            Self::Pending(ref inner) => {
                output.write_all(&[0x00])?;
                inner.write_into(output)?;
            }
            Self::Committed(ref inner) => {
                output.write_all(&[0x01])?;
                inner.write_into(output)?;
            }
            Self::Failed => {
                output.write_all(&[0xff])?;
            }
        }
        Ok(())
    }
}

impl Into<u8> for CellStatus {
    fn into(self) -> u8 {
        match self {
            Self::Live => 0b00,
            Self::Burn => 0b10,
            Self::Dead => 0b11,
        }
    }
}

impl TryFrom<u8> for CellStatus {
    type Error = Error;
    fn try_from(value: u8) -> Result<Self> {
        let ret = match value {
            0b00 => Self::Live,
            0b10 => Self::Burn,
            0b11 => Self::Dead,
            x => {
                let errmsg = format!("cell status is unknown [{}]", x);
                return Err(Error::broken_since("CellStatus", &errmsg));
            }
        };
        Ok(ret)
    }
}

impl TxOutputsStatus {
    const NAME: &'static str = "TxOutputsStatus";

    fn new_all_live(count: usize) -> Self {
        let statuses = vec![CellStatus::Live; count];
        Self { statuses }
    }

    pub(crate) fn count(&self) -> usize {
        self.statuses.len()
    }

    pub(crate) fn status(&self, index: usize) -> &CellStatus {
        &self.statuses[index]
    }

    fn is_invalid(&self) -> bool {
        !self.statuses.iter().any(|st| st == &CellStatus::Live)
    }

    fn spent(&mut self, index: usize) {
        if self.statuses[index] != CellStatus::Live {
            panic!("the cell should be live before spent");
        }
        self.statuses[index] = CellStatus::Dead;
    }

    fn from_slice(slice: &[u8]) -> Result<Self> {
        let count = read_u32(slice)? as usize;
        let expected = 4 + (count + 3) / 4;
        if slice.len() != expected {
            let reason = format!(
                "incorrect data size (expect: {}, actual: {})",
                expected,
                slice.len()
            );
            return Err(Error::broken_since(Self::NAME, &reason));
        }
        let mut statuses = (&slice[4..])
            .iter()
            .map(|value| {
                let v0 = CellStatus::try_from((value >> 6) & 0b11)?;
                let v1 = CellStatus::try_from((value >> 4) & 0b11)?;
                let v2 = CellStatus::try_from((value >> 2) & 0b11)?;
                let v3 = CellStatus::try_from((value) & 0b11)?;
                Ok(vec![v0, v1, v2, v3])
            })
            .collect::<Result<Vec<Vec<CellStatus>>>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<CellStatus>>();
        if statuses[count..].iter().any(|st| *st != CellStatus::Live) {
            return Err(Error::broken_since(
                Self::NAME,
                "all redundant part should be default value",
            ));
        }
        statuses.truncate(count);
        Ok(Self { statuses })
    }

    fn write_into<W: io::Write>(&self, output: &mut W) -> StdResult<(), io::Error> {
        write_u32(output, self.statuses.len() as u32)?;
        let statuses_bytes = self
            .statuses
            .chunks(4)
            .map(|slice| {
                let mut ret: u8 = 0;
                for (index, status) in slice.iter().enumerate() {
                    let value: u8 = (*status).into();
                    ret |= value << ((3 - index) * 2);
                }
                ret
            })
            .collect::<Vec<_>>();
        output.write_all(&statuses_bytes)?;
        Ok(())
    }
}

impl fmt::Display for CacheStats {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(
            f,
            "tx.pending: {}, tx.committed: {}, tx.failed: {}, cell.live: {}",
            self.tx_pending_cnt(),
            self.tx_committed_cnt(),
            self.tx_failed_cnt(),
            self.cell_live_cnt()
        )
    }
}

impl CacheStats {
    pub(crate) fn tx_pending_cnt(&self) -> usize {
        self.tx_pending_cnt
    }

    pub(crate) fn tx_committed_cnt(&self) -> usize {
        self.tx_committed_cnt
    }

    pub(crate) fn tx_failed_cnt(&self) -> usize {
        self.tx_failed_cnt
    }

    pub(crate) fn cell_live_cnt(&self) -> usize {
        self.cell_live_cnt
    }

    pub(crate) fn submit_tx(&mut self, inputs_count: usize, tx_status: &TxStatus) -> Result<()> {
        self.tx_pending_cnt += 1;
        self.cell_live_cnt -= inputs_count;
        match tx_status {
            TxStatus::Pending(ref inner) | TxStatus::Committed(ref inner) => {
                self.load_cells(&inner.statuses);
            }
            TxStatus::Failed => {
                self.tx_failed_cnt += 1;
            }
        }
        Ok(())
    }

    pub(crate) fn remove_tx(&mut self, tx_status: &TxStatus) {
        match tx_status {
            TxStatus::Pending(..) => {
                self.tx_pending_cnt -= 1;
            }
            TxStatus::Committed(..) => {
                self.tx_committed_cnt -= 1;
            }
            TxStatus::Failed => {
                self.tx_failed_cnt -= 1;
            }
        }
    }

    pub(crate) fn commit_cellbase(&mut self, outputs_count: usize) {
        self.tx_committed_cnt += 1;
        self.cell_live_cnt += outputs_count;
    }

    pub(crate) fn commit_pending(&mut self) {
        self.tx_pending_cnt -= 1;
        self.tx_committed_cnt += 1;
    }

    pub(crate) fn load_tx(&mut self, tx_status: &TxStatus) {
        match tx_status {
            TxStatus::Pending(ref inner) => {
                self.tx_pending_cnt += 1;
                self.load_cells(&inner.statuses);
            }
            TxStatus::Committed(ref inner) => {
                self.tx_committed_cnt += 1;
                self.load_cells(&inner.statuses);
            }
            TxStatus::Failed => {
                self.tx_failed_cnt += 1;
            }
        }
    }

    fn load_cells(&mut self, statuses: &[CellStatus]) {
        let live_cnt = statuses
            .iter()
            .filter(|st| matches!(st, CellStatus::Live))
            .count();
        self.cell_live_cnt += live_cnt;
    }
}

fn write_u32<W: io::Write>(output: &mut W, num: u32) -> StdResult<(), io::Error> {
    let num_bytes = num.to_le_bytes();
    output.write_all(&num_bytes)?;
    Ok(())
}

fn read_u32(slice: &[u8]) -> Result<u32> {
    if slice.len() < 4 {
        return Err(Error::broken_since("u32", "no enough data"));
    }
    let mut b = [0u8; 4];
    b.copy_from_slice(&slice[..4]);
    Ok(u32::from_le_bytes(b))
}
