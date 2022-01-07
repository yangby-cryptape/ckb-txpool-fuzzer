use std::env;

use crate::error::{Error, Result};

pub(crate) fn enable() -> Result<()> {
    let faketime_file = faketime::millis_tempfile(0).map_err(|err| {
        let errmsg = format!("failed to create faketime tempfile since {}", err);
        Error::Runtime(errmsg)
    })?;
    env::set_var("FAKETIME", faketime_file.as_os_str());
    drop(faketime_file);
    Ok(())
}

pub(crate) fn update(timestamp_millis: u64) -> Result<()> {
    env::var("FAKETIME")
        .map_err(|err| {
            let errmsg = format!("failed to read env \"FAKETIME\" since {}", err);
            Error::Runtime(errmsg)
        })
        .and_then(|faketime_file| {
            faketime::write_millis(faketime_file, timestamp_millis).map_err(|err| {
                let errmsg = format!("failed to update faketime since {}", err);
                Error::Runtime(errmsg)
            })
        })
}

pub(crate) fn increase(millis: u32) -> Result<()> {
    let prev_timestamp_millis = faketime::unix_time_as_millis();
    update(prev_timestamp_millis + u64::from(millis))
}
