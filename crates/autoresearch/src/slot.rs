//! W1 MEASURE — the machine bench slot and the load precondition (design doc
//! 140 §5.5): `bench.lock = builtin` is a mkdir slot under
//! `/tmp/theuth-bench-slots/0` with an owner file `pid label epoch argv` (the
//! build-lock layout); any other value is a wrapper command the sample runs
//! through. Load = 1-minute average ÷ cores, waited for up to `bench.wait`,
//! then the sample runs flagged `noisy`.

use std::time::Duration;

use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};

/// A held bench slot; released on drop.
#[derive(Debug)]
pub struct Slot {
    _label: String,
}

/// Take the builtin slot for `label`, waiting until it is free or `cancel` fires.
pub async fn acquire(
    _cfg: &crate::config::Bench,
    _label: &str,
    _cancel: &CancellationToken,
) -> Result<Slot> {
    Err(Error::NotBuilt("W1"))
}

/// 1-minute load average ÷ cores now.
pub fn load() -> Option<f64> {
    // stub: W1
    None
}

/// Wait until `load() <= max_load` or `wait` elapses; `Ok(true)` = the
/// precondition held, `Ok(false)` = ran out of patience (sample is `noisy`).
pub async fn wait_for_load(
    _max_load: f64,
    _wait: Duration,
    _cancel: &CancellationToken,
) -> Result<bool> {
    Err(Error::NotBuilt("W1"))
}

/// Who holds the build slots right now (`/tmp/theuth-build-slots/*/owner`), for the sample row.
pub fn build_slot_holders() -> Vec<String> {
    // stub: W1
    Vec::new()
}
