use std::{
    cell::{RefCell, RefMut},
    ops::DerefMut as _,
};

use rand::{rngs::ThreadRng, thread_rng, Rng as _};
use rand_distr::{Distribution as _, Normal};

use crate::{
    error::{Error, Result},
    types::RunEnv,
};

pub(crate) struct RandomGenerator {
    rng: RefCell<ThreadRng>,
    block_interval: Normal<f64>,
}

impl RandomGenerator {
    pub(crate) fn new(run_env: &RunEnv) -> Result<Self> {
        let rng = RefCell::new(thread_rng());
        let block_interval = {
            let mean = f64::from(run_env.block_interval);
            let std_dev = mean / 4.0;
            Normal::new(mean, std_dev).map_err(Error::runtime)
        }?;
        Ok(Self {
            rng,
            block_interval,
        })
    }

    fn rng(&self) -> RefMut<ThreadRng> {
        self.rng.borrow_mut()
    }

    pub(crate) fn block_interval(&self) -> u32 {
        let mut ret;
        loop {
            ret = self.block_interval.sample(self.rng().deref_mut());
            if ret > 0.0 {
                break;
            }
        }
        ret.ceil() as u32
    }

    pub(crate) fn random_hash(&self) -> [u8; 32] {
        let mut hash = [0u8; 32];
        self.rng().deref_mut().fill(&mut hash[..]);
        hash
    }

    // 9/10 chance to add another tx.
    pub(crate) fn has_next_transaction(&self) -> bool {
        self.rng().deref_mut().gen_range::<u32, _>(0..10) > 0
    }

    // 1/1000 chance to generate an empty inputs transaction.
    pub(crate) fn no_inputs(&self) -> bool {
        self.rng().deref_mut().gen_range::<u32, _>(0..1000) == 0
    }

    // 1/1000 chance to generate an empty outputs transaction.
    pub(crate) fn no_outputs(&self) -> bool {
        self.rng().deref_mut().gen_range::<u32, _>(0..1000) == 0
    }

    // 1/1000 chance to overflow the total capacity
    pub(crate) fn allow_capacity_overflow(&self) -> bool {
        self.rng().deref_mut().gen_range::<u32, _>(0..1000) == 0
    }

    // 7/8 chance to add another input cell.
    pub(crate) fn has_next_input(&self) -> bool {
        self.rng().deref_mut().gen_range::<u32, _>(0..7) > 0
    }

    // 1/200 chance to add a burned cell as input.
    pub(crate) fn could_has_burned_input(&self) -> bool {
        self.rng().deref_mut().gen_range::<u32, _>(0..200) == 0
    }

    // 1/200 chance to add a dead cell as input.
    pub(crate) fn could_has_dead_input(&self) -> bool {
        self.rng().deref_mut().gen_range::<u32, _>(0..200) == 0
    }

    // 1/200 chance to add a cell from a failed transaction.
    pub(crate) fn could_be_from_failed_tx(&self) -> bool {
        self.rng().deref_mut().gen_range::<u32, _>(0..200) == 0
    }

    // 1/200 chance to allow duplicated cell.
    pub(crate) fn allow_duplicated(&self) -> bool {
        self.rng().deref_mut().gen_range::<u32, _>(0..200) == 0
    }

    // Lock Script:
    // - 1/100 chance: no lock script
    // - 10/100 chance: failed lock script
    pub(crate) fn lock_status(&self) -> Option<bool> {
        let tmp = self.rng().deref_mut().gen_range::<u32, _>(0..100);
        if tmp == 0 {
            None
        } else if tmp < 10 {
            Some(false)
        } else {
            Some(true)
        }
    }

    // Type Script:
    // - 40/100 chance: no type script
    // - 10/100 chance: failed type script
    pub(crate) fn type_status(&self) -> Option<bool> {
        let tmp = self.rng().deref_mut().gen_range::<u32, _>(0..100);
        if tmp < 40 {
            None
        } else if tmp < 10 {
            Some(false)
        } else {
            Some(true)
        }
    }

    // 40/100 chance: data hash-type
    // 60/100 chance: type hash-type
    pub(crate) fn is_data_hash_type(&self) -> bool {
        self.rng().deref_mut().gen_range::<u32, _>(0..100) < 40
    }

    pub(crate) fn usize_less_than(&self, limit: usize) -> usize {
        self.rng().deref_mut().gen_range::<usize, _>(0..limit)
    }

    pub(crate) fn u64_between(&self, smallest: u64, limit: u64) -> u64 {
        self.rng().deref_mut().gen_range(smallest..limit)
    }
}
