//! omp adapter (141 §6.4, D7): the two engine-side feeds for omp's `task`
//! subagents — *live* progress rows riding the parent's `tool_call_update`
//! frames ([`progress_rows`]) and the *tail* of the child's on-disk transcript
//! ([`Tailer`], [`to_updates`]). The acp crate exposes the adapter; the
//! `AcpChild` actor calls it (141 §7.3).
//!
//! Fixtures: `crates/acp/fixtures/omp-18.3.1/` — frames 8/12/27/44 of the
//! live capture (`task.{tool_call, completed_stale, mid, final}.json`), the
//! child transcript `ReadA.jsonl` with its `.md`/`.json` sidecars, and the
//! parent's `async-result` entry (`parent.jsonl`), prose trimmed.

mod progress;
mod tail;

pub use progress::{Progress, RecentTool, RetryState, TaskFrame, progress_rows};
pub use tail::{Call, Entry, Tailer, Usage, to_updates};
