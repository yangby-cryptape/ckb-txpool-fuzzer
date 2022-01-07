use std::{collections::HashMap, fmt};

use ckb_store::ChainStore as _;
use ckb_types::{core, packed, prelude::*};

use super::{MockedChain, Overlay, Storage, TxOverlay, TxOverlayChanges};
use crate::{
    error::Result,
    types::{CellStatus, RandomGenerator, ScriptAnchor, TxOutputsStatus, TxStatus},
};

const BYTE_SHANNONS: u64 = 100_000_000;
const SMALLEST_SHANNONS: u64 = 138 * BYTE_SHANNONS;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Pending,
    Committed,
    Failed,
}

struct RawInputCell {
    tx_hash: packed::Byte32,
    index: usize,
    status: Status,
}

struct InputCell {
    tx_hash: packed::Byte32,
    index: u32,
    status: Status,
    capacity: core::Capacity,
}

struct RawOutputCell {
    output: packed::CellOutput,
    data_size: usize,
    cell_status: CellStatus,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Committed => write!(f, "committed"),
            Self::Failed => write!(f, "failed"),
        }
    }
}

impl Status {
    fn merge(self, another: Self) -> Self {
        if self == Self::Failed || another == Self::Failed {
            Self::Failed
        } else if self == Self::Pending || another == Self::Pending {
            Self::Pending
        } else {
            Self::Committed
        }
    }
}

impl RawInputCell {
    fn new(tx_hash: packed::Byte32, index: usize, status: Status) -> Self {
        Self {
            tx_hash,
            index,
            status,
        }
    }
}

impl RawOutputCell {
    fn new(output: packed::CellOutput, data_size: usize, cell_status: CellStatus) -> Self {
        Self {
            output,
            data_size,
            cell_status,
        }
    }
}

pub(crate) fn build_transactions(
    rg: &RandomGenerator,
    chain: &MockedChain,
    storage: &Storage,
) -> Result<Vec<TxOverlay>> {
    let mut overlay = Overlay::new(storage);
    while rg.has_next_transaction() {
        log::trace!("[BuildTx] try to generate one more transaction");
        if let Some(tx) = generate_transaction(rg, chain, &overlay)? {
            let tx_view = tx.view();
            log::trace!(
                "[BuildTx] the new transaction is {:#x} ({} -> {}, {:?})",
                tx_view.hash(),
                tx_view.inputs().len(),
                tx_view.outputs().len(),
                tx.status(),
            );
            if overlay.has_tx(&tx_view.hash()) {
                break;
            }
            overlay.add_tx(tx);
        } else {
            break;
        }
    }
    Ok(overlay.txs.into_values().collect())
}

pub(crate) fn generate_transaction(
    rg: &RandomGenerator,
    chain: &MockedChain,
    overlay: &Overlay,
) -> Result<Option<TxOverlay>> {
    // Waiting for enough cells.
    let live_cells_count = overlay.live_cells_count();
    if live_cells_count < 1_000 {
        log::trace!(
            "[BuildTx] >>> live cells (size: {}) is not enough",
            live_cells_count
        );
        return Ok(None);
    }
    let inputs = generate_inputs(rg, overlay);
    let inputs_status = if inputs.is_empty() {
        Status::Failed
    } else {
        inputs
            .iter()
            .fold(Status::Committed, |all, next| all.merge(next.status))
    };
    log::trace!(
        "[BuildTx] >>> generate {} input cells (expected: {})",
        inputs.len(),
        inputs_status
    );
    let inputs = complete_inputs(chain, overlay, inputs);
    {
        let inputs_count = inputs.len();
        for (index, item) in inputs.iter().enumerate() {
            log::trace!(
                "[BuildTx] >>> >>> spend ({:6}/{:6}) {:#x},{} (status: {})",
                index,
                inputs_count,
                item.tx_hash,
                item.index,
                item.status,
            );
        }
    }
    let mocked_script = chain.mocked_script();
    let (outputs, outputs_status) = generate_outputs(rg, &inputs, &mocked_script);
    log::trace!(
        "[BuildTx] >>> generate {} output cells (expected: {})",
        outputs.len(),
        outputs_status
    );
    let tx_view = {
        let inputs = inputs.iter().map(|item| {
            let op = packed::OutPoint::new(item.tx_hash.to_owned(), item.index);
            packed::CellInput::new(op, 0)
        });
        let (outputs, outputs_data) = outputs.iter().fold(
            (Vec::new(), Vec::new()),
            |(mut outputs, mut outputs_data), item| {
                outputs.push(item.output.to_owned());
                outputs_data.push(vec![0u8; item.data_size].pack());
                (outputs, outputs_data)
            },
        );
        core::TransactionView::new_advanced_builder()
            .cell_dep(mocked_script.cell_dep())
            .inputs(inputs)
            .outputs(outputs)
            .outputs_data(outputs_data)
            .build()
    };
    let changes = {
        let final_status = inputs_status.merge(outputs_status);
        let new = {
            let statuses = outputs
                .iter()
                .map(|raw| raw.cell_status)
                .collect::<Vec<_>>();
            TxOutputsStatus { statuses }
        };
        match final_status {
            Status::Pending => {
                let mut updates = HashMap::new();
                for input in &inputs {
                    if input.status == Status::Failed {
                        panic!("All input cells should be available.")
                    }
                    let tx_status = overlay.get_tx_status(&input.tx_hash)?;
                    updates
                        .entry(input.tx_hash.to_owned())
                        .or_insert(tx_status)
                        .spent(input.index as usize);
                }
                TxOverlayChanges::Pending { new, updates }
            }
            Status::Committed => {
                let mut updates = HashMap::new();
                for input in &inputs {
                    if input.status == Status::Failed {
                        panic!("All input cells should be available.")
                    }
                    let tx_status = overlay.get_tx_status(&input.tx_hash)?;
                    updates
                        .entry(input.tx_hash.to_owned())
                        .or_insert(tx_status)
                        .spent(input.index as usize);
                }
                TxOverlayChanges::Committed { new, updates }
            }
            Status::Failed => {
                let mut updates = HashMap::new();
                for input in &inputs {
                    if input.status == Status::Failed {
                        let tx_status = overlay.get_tx_status(&input.tx_hash)?;
                        if tx_status.is_invalid() {
                            updates.entry(input.tx_hash.to_owned()).or_insert(tx_status);
                        }
                    }
                }
                TxOverlayChanges::Failed { updates }
            }
        }
    };
    Ok(Some(TxOverlay::new(tx_view, changes)))
}

fn generate_inputs(rg: &RandomGenerator, overlay: &Overlay) -> Vec<RawInputCell> {
    let mut inputs = Vec::new();
    if rg.no_inputs() {
        return inputs;
    }
    'found_inputs: loop {
        if !inputs.is_empty() && !rg.has_next_input() {
            break;
        }
        let cell_opt;
        'loop_cells: loop {
            let random_tx = overlay.random_tx(rg).unwrap();
            if random_tx.is_none() {
                break 'found_inputs;
            }
            let (tx_hash, tx_status) = random_tx.unwrap();
            match tx_status {
                TxStatus::Pending(ref cells) | TxStatus::Committed(ref cells) => {
                    let status = match tx_status {
                        TxStatus::Pending(_) => Status::Pending,
                        TxStatus::Committed(_) => Status::Committed,
                        _ => unreachable!(),
                    };
                    let cells_count = cells.count();
                    let cell_index_start = rg.usize_less_than(cells_count);
                    for cell_index in (cell_index_start..cells_count)
                        .into_iter()
                        .chain((0..cell_index_start).into_iter())
                    {
                        match cells.status(cell_index) {
                            CellStatus::Live => {
                                cell_opt =
                                    Some(RawInputCell::new(tx_hash.to_owned(), cell_index, status));
                                break 'loop_cells;
                            }
                            CellStatus::Burn => {
                                if rg.could_has_burned_input() {
                                    cell_opt = Some(RawInputCell::new(
                                        tx_hash.to_owned(),
                                        cell_index,
                                        Status::Failed,
                                    ));
                                    break 'loop_cells;
                                }
                            }
                            CellStatus::Dead => {
                                if rg.could_has_dead_input() {
                                    cell_opt = Some(RawInputCell::new(
                                        tx_hash.to_owned(),
                                        cell_index,
                                        Status::Failed,
                                    ));
                                    break 'loop_cells;
                                }
                            }
                        }
                    }
                }
                TxStatus::Failed => {
                    if rg.could_be_from_failed_tx() {
                        cell_opt = Some(RawInputCell::new(tx_hash.to_owned(), 0, Status::Failed));
                        break 'loop_cells;
                    }
                }
            }
        }
        if let Some(mut cell) = cell_opt {
            if !inputs
                .iter()
                .any(|item| item.tx_hash == cell.tx_hash && item.index == cell.index)
            {
                inputs.push(cell);
            } else if rg.allow_duplicated() {
                cell.status = Status::Failed;
                inputs.push(cell);
            }
        }
    }
    inputs
}

fn complete_inputs(
    chain: &MockedChain,
    overlay: &Overlay,
    raw_cells: Vec<RawInputCell>,
) -> Vec<InputCell> {
    raw_cells
        .into_iter()
        .map(|raw| {
            let index = raw.index as u32;
            let outputs = if let Some(tx_view) = overlay.get_tx(&raw.tx_hash) {
                tx_view
            } else {
                chain
                    .store()
                    .get_transaction(&raw.tx_hash)
                    .map(|(tx, _)| tx)
                    .unwrap()
            }
            .outputs();
            let capacity = if let Some(output) = outputs.get(raw.index) {
                output.capacity().unpack()
            } else {
                core::Capacity::shannons(SMALLEST_SHANNONS)
            };
            InputCell {
                tx_hash: raw.tx_hash,
                index,
                status: raw.status,
                capacity,
            }
        })
        .collect()
}

fn generate_outputs(
    rg: &RandomGenerator,
    inputs: &[InputCell],
    mocked_script: &ScriptAnchor,
) -> (Vec<RawOutputCell>, Status) {
    let mut expected_status = Status::Failed;
    let mut outputs = Vec::new();
    if inputs.is_empty() || rg.no_outputs() {
        log::trace!("[BuildTx] >>> >>> failed since: inputs or outputs is empty");
        return (outputs, expected_status);
    }
    // TODO Random fee base on the fee rate.
    let fee = core::Capacity::shannons(10_000_000);
    let total_capacity = inputs
        .iter()
        .map(|item| item.capacity)
        .try_fold(core::Capacity::zero(), core::Capacity::safe_add)
        .unwrap();
    if total_capacity < fee {
        log::trace!("[BuildTx] >>> >>> failed since: no enough fee");
        return (outputs, expected_status);
    }
    let remain_capacity = total_capacity.safe_sub(fee).unwrap();
    if remain_capacity.as_u64() < SMALLEST_SHANNONS {
        log::trace!("[BuildTx] >>> >>> failed since: no enough capacity");
        return (outputs, expected_status);
    }
    let mut remain_shannons = {
        if rg.allow_capacity_overflow() {
            log::trace!("[BuildTx] >>> >>> failed since: capacity overflow");
            expected_status = Status::Failed;
            let one_shannon = core::Capacity::shannons(1);
            total_capacity.safe_add(one_shannon).unwrap()
        } else {
            expected_status = Status::Pending;
            remain_capacity
        }
    }
    .as_u64();
    loop {
        if remain_shannons == 0 {
            break;
        }
        let output_shannons = {
            let mut shannons = if remain_shannons == SMALLEST_SHANNONS {
                remain_shannons
            } else {
                rg.u64_between(SMALLEST_SHANNONS, remain_shannons)
            };
            remain_shannons -= shannons;
            if remain_shannons < SMALLEST_SHANNONS {
                shannons += remain_shannons;
                remain_shannons = 0;
            }
            shannons
        };
        let lock_status = rg.lock_status();
        let cell_status = if lock_status.unwrap_or(false) {
            CellStatus::Live
        } else {
            CellStatus::Burn
        };
        let lock_script = match lock_status {
            None => packed::Script::default(),
            Some(inner) => generate_script(rg, mocked_script, inner),
        };
        let type_status = rg.type_status();
        let status = if matches!(type_status, Some(false)) {
            log::trace!("[BuildTx] >>> >>> failed since: type script");
            Status::Failed
        } else {
            Status::Pending
        };
        expected_status = expected_status.merge(status);
        let type_script_opt = type_status.map(|inner| generate_script(rg, mocked_script, inner));
        let output = {
            let tmp_output = packed::CellOutput::new_builder()
                .lock(lock_script)
                .type_(type_script_opt.pack())
                .build_exact_capacity(core::Capacity::zero())
                .unwrap();
            let tmp_shannons: u64 = tmp_output.capacity().unpack();
            let free_bytes = ((output_shannons - tmp_shannons) / BYTE_SHANNONS) as usize;
            let data_size = if free_bytes > 0 {
                rg.usize_less_than(free_bytes)
            } else {
                0
            };
            let output = tmp_output
                .as_builder()
                .capacity(core::Capacity::shannons(output_shannons).pack())
                .build();
            RawOutputCell::new(output, data_size as usize, cell_status)
        };
        outputs.push(output);
    }
    (outputs, expected_status)
}

fn generate_script(
    rg: &RandomGenerator,
    mocked_script: &ScriptAnchor,
    result: bool,
) -> packed::Script {
    let result: u64 = if result { 0 } else { 1 };
    let cycles: u64 = rg.u64_between(500, 1_000_000);
    let (hash_type, code_hash) = if rg.is_data_hash_type() {
        (core::ScriptHashType::Data, mocked_script.data_hash())
    } else {
        (core::ScriptHashType::Type, mocked_script.type_hash())
    };
    let args = {
        let mut tmp = vec![0u8; 32];
        let result_bytes = result.to_le_bytes();
        let cycles_bytes = cycles.to_le_bytes();
        (&mut tmp[0..8]).copy_from_slice(&result_bytes);
        (&mut tmp[8..16]).copy_from_slice(&cycles_bytes);
        (&mut tmp[16..24]).copy_from_slice(&result_bytes);
        (&mut tmp[24..32]).copy_from_slice(&cycles_bytes);
        tmp
    };
    packed::Script::new_builder()
        .hash_type(hash_type.into())
        .code_hash(code_hash)
        .args(args.pack())
        .build()
}
