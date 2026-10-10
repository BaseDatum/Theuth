//! 158 W3 owns this file (stub from C158).
//!
//! Projects as the meeting series (158 §9): the meeting ↔ project field and its RPC, the
//! `cal_project_memory` lookup order (`uid:` → `title:` → `meeting.project` → none), and the
//! pre-meeting brief — one Private writer turn on `meeting.model`, cached in `cal_prebrief`,
//! surfaced as `MeetingEvent::Upcoming` from `start − 5 min`. The stub writes nothing and
//! never creates a session.

use runtime::Runtime;

use super::EventRef;
use crate::{Manifest, MeetingId};

/// `meeting_set_project(id, project) -> Manifest` (§9.2, W3): manifest + index row +
/// `agent::SetSessionProject` + the memory rows (cleared when `None`). Stub: refused.
pub async fn rpc_set_project(
    _rt: &Runtime,
    _id: MeetingId,
    _project: Option<String>,
) -> anyhow::Result<Manifest> {
    Err(anyhow::anyhow!("158 W3: not built yet"))
}

/// §9.3 lookup for an event (or a bare title when `event` is `None`): the remembered project,
/// else `meeting.project`, else `None`. Stub: nothing remembered.
pub async fn remembered_project(
    _rt: &Runtime,
    _event: Option<&EventRef>,
    _title: Option<&str>,
) -> anyhow::Result<Option<String>> {
    Ok(None)
}

/// §9.4: the cached bullets for `event` in `project`, generating them when missing and
/// `meeting.calendar.prebrief` is on. Stub: no brief.
pub async fn brief_for(
    _rt: &Runtime,
    _event: &EventRef,
    _project: &str,
) -> anyhow::Result<Option<Vec<String>>> {
    Ok(None)
}
