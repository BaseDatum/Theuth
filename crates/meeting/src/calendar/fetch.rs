//! 158 W2 owns this file (stub from C158).
//!
//! ICS subscription fetch (158 §7.3): GET the vault URL `calendar.ics.<id>` with `If-None-Match`
//! / `If-Modified-Since`, errors redacted through `Vault::redact` (the URL is never logged),
//! every `meeting.calendar.poll_secs`. The stub never touches the network.

use runtime::Runtime;

use super::{CalEvent, CalSource};

/// Fetch and expand one subscription for the store's window; `Ok(None)` = not modified.
/// Stub: refused.
pub async fn fetch(_rt: &Runtime, _source: &CalSource) -> anyhow::Result<Option<Vec<CalEvent>>> {
    Err(anyhow::anyhow!("158 W2: not built yet"))
}
