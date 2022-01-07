use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use crate::error::{Error, Result};

pub(crate) fn capture() -> Result<Arc<AtomicBool>> {
    let pressed = Arc::new(AtomicBool::new(false));
    let ctrlc_flag = Arc::clone(&pressed);
    ctrlc::set_handler(move || {
        ctrlc_flag.store(true, Ordering::SeqCst);
    })
    .map_err(|err| {
        let errmsg = format!("failed to set Ctrl-C handler since {}", err);
        Error::runtime(errmsg)
    })?;
    Ok(pressed)
}
