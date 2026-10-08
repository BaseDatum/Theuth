//! 137 Revision 1 seams the loop, the spawn path and the RPC call, written
//! against the interfaces W1 adds to [`Endless`] in a follow-up
//! (`render_snapshot`, `render_with_budget`, `UnitFilter` on `set_mode`,
//! `set_view_bytes`, `backfill_estimate`). Until then each function is a thin
//! adapter over the contract's v1 API; when the service grows the method,
//! the body becomes a one-line delegation and nothing else moves.

use runtime::SessionId;

use serde::{Deserialize, Serialize};

use crate::endless::{Backfill, Endless, MemoryMode, SessionMemory, SetMode, ViewBlock, ViewLine};

/// Which units a tree is built over (R1-D6): everything, or a meeting's
/// transcript batches and topic/brief tool calls only (W4's).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnitFilter {
    /// Every unit (chats, desks).
    #[default]
    All,
    /// Transcript batches and topic/brief tool calls only.
    Meeting,
}

/// What `/endless on` tells the user before the backfill runs (R1-D3, R1-D4).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BackfillEstimate {
    /// Units still to summarise.
    pub units: u64,
    /// Wall-clock estimate for the backfill.
    pub eta_secs: u64,
    /// Estimated cost in USD; `None` until the service measures its first nodes.
    pub usd: Option<f64>,
    /// The compactor model id (R1-D3 chain, resolved by the service).
    pub model: String,
}

/// R1-D1: the parent's rendered view as cache-marked blocks for a native
/// child's first message. Adapter: rendered from the panel lines of
/// [`Endless::memory`] as one `<chat>` block (the service's `render_snapshot`
/// will return the band-split blocks without touching the frozen context).
/// Empty when the parent has no view (not endless, or nothing summarised).
pub async fn render_snapshot(endless: &Endless, session: SessionId) -> Vec<ViewBlock> {
    let Ok(m) = endless.memory(session).await else {
        return Vec::new();
    };
    if m.mode != MemoryMode::Endless || m.view.is_empty() {
        return Vec::new();
    }
    let mut text = String::from("<chat>\n");
    for l in &m.view {
        text.push_str(&format!("{}+{}|{}\n", l.id, l.n, l.text.replace('\n', " ")));
    }
    text.push_str("</chat>");
    vec![ViewBlock { text, mark: true }]
}

/// R1-D6: the view folded to a `bytes` budget (a meeting head's "Earlier in
/// this meeting" section). Adapter: the live view's lines, trimmed from the
/// oldest end to fit `bytes` (the service's `render_with_budget` will fold
/// the tree to the budget instead of cutting lines); unbuilt parts render as
/// the service renders them.
pub async fn render_with_budget(
    endless: &Endless,
    session: SessionId,
    bytes: u32,
) -> anyhow::Result<Vec<ViewLine>> {
    let m = endless.memory(session).await?;
    if m.mode != MemoryMode::Endless {
        return Ok(Vec::new());
    }
    let budget = bytes as usize;
    let mut total: usize = m.view.iter().map(|l| l.text.len()).sum();
    let mut skip = 0;
    while total > budget && skip < m.view.len() {
        total -= m.view[skip].text.len();
        skip += 1;
    }
    if skip > 0 {
        tracing::debug!(session = %session, bytes, dropped = skip, "endless render_with_budget: adapter cut the oldest lines");
    }
    Ok(m.view.into_iter().skip(skip).collect())
}

/// `set_mode` with a unit filter (R1-D6). Adapter: the filter is recorded in
/// the log only; the service builds over every unit.
pub async fn set_mode(
    endless: &Endless,
    session: SessionId,
    mode: MemoryMode,
    epoch: u32,
    filter: UnitFilter,
) -> anyhow::Result<SetMode> {
    if filter != UnitFilter::All {
        tracing::info!(session = %session, ?filter, "endless set_mode: unit filter not yet honoured by the service");
    }
    endless.set_mode(session, mode, epoch).await
}

/// R1-D3/R1-D4: the estimate `/endless on` replies with, from the state the
/// switch returned. Adapter: seconds from the service's `backfill.eta_secs`,
/// cost unknown until the service exposes `backfill_estimate`.
pub fn backfill_estimate(m: &SessionMemory) -> BackfillEstimate {
    let (units, eta_secs) = match m.backfill {
        Some(Backfill {
            done,
            total,
            eta_secs,
        }) => (total.saturating_sub(done), eta_secs),
        None => (0, 0),
    };
    BackfillEstimate {
        units,
        eta_secs,
        usd: None,
        model: m.model.clone(),
    }
}

/// R1-D2b: the per-session view budget. Valid range 16_000..=256_000.
/// Adapter: validated and logged; the service's `set_view_bytes` (and
/// `SessionMemory.view_bytes`) will make it take effect at the next turn.
pub async fn set_view_bytes(
    endless: &Endless,
    session: SessionId,
    bytes: u32,
) -> Result<SessionMemory, String> {
    if !(16_000..=256_000).contains(&bytes) {
        return Err(format!("view_bytes {bytes} out of range 16000..=256000"));
    }
    tracing::info!(session = %session, bytes, "endless set_view_bytes: accepted; applied by the service at the next turn boundary");
    endless.memory(session).await.map_err(|e| format!("{e:#}"))
}

/// `Nm Ns` / `Nh Nm` for an ETA.
pub fn eta(secs: u64) -> String {
    match secs {
        0 => "under a minute".to_owned(),
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {:02}s", s / 60, s % 60),
        s => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// The text every `/endless on|off` surface replies with (8.6, R1-D3, R1-D4),
/// from the state the switch returned.
pub fn reply(mode: MemoryMode, m: &SessionMemory) -> String {
    match mode {
        MemoryMode::Endless => {
            let est = backfill_estimate(m);
            let model = if est.model.is_empty() {
                "the session's model".to_owned()
            } else {
                est.model.clone()
            };
            if est.units == 0 {
                format!("Endless memory on. Compactor: {model}. The view is up to date.")
            } else {
                let usd = est
                    .usd
                    .map_or(String::new(), |u| format!(", ≈ ${u:.2}"));
                format!(
                    "Endless memory on. Summarising {} messages in the background with {model} (ETA {}{usd}); the session keeps its current memory until the view covers its history.",
                    est.units,
                    eta(est.eta_secs)
                )
            }
        }
        MemoryMode::Epochs => {
            "Endless memory off. The view was written as this epoch's summary; the session continues with epochs.".to_owned()
        }
    }
}

/// One line of memory state for `/endless` with no argument (and the
/// header badge's tooltip): mode, view size, lines, settle state, backfill.
pub fn status(m: &SessionMemory) -> String {
    if m.mode != MemoryMode::Endless {
        return "Memory: epochs (compaction). `/endless on` switches this session to endless memory."
            .to_owned();
    }
    let bytes: usize = m.view.iter().map(|l| l.text.len()).sum();
    let mut s = format!(
        "Memory: endless · view {} KB · {} lines · {} messages · compactor {}",
        bytes / 1000,
        m.view.len(),
        m.units,
        if m.model.is_empty() {
            "session model"
        } else {
            &m.model
        }
    );
    if let Some(b) = m.backfill {
        s.push_str(&format!(
            " · backfill {}/{} (ETA {})",
            b.done,
            b.total,
            eta(b.eta_secs)
        ));
    } else if m.settling {
        s.push_str(&format!(" · settling {} lines", m.unbuilt));
    } else {
        s.push_str(" · up to date");
    }
    if m.stats.calls > 0 {
        s.push_str(&format!(
            " · {} compactor calls, ≈ ${:.2}",
            m.stats.calls, m.stats.usd_estimate
        ));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::endless::{EndlessStats, ViewLine};

    fn memory(backfill: Option<Backfill>) -> SessionMemory {
        SessionMemory {
            mode: MemoryMode::Endless,
            view: vec![ViewLine {
                id: 0,
                n: 2,
                level: 1,
                text: "user: a\nb".into(),
                built: true,
            }],
            settling: false,
            unbuilt: 0,
            units: 2,
            stats: EndlessStats::default(),
            model: "p/haiku".into(),
            backfill,
        }
    }

    #[test]
    fn estimate_and_reply_follow_the_backfill_state() {
        let fresh = backfill_estimate(&memory(None));
        assert_eq!((fresh.units, fresh.eta_secs), (0, 0));
        assert!(reply(MemoryMode::Endless, &memory(None)).contains("up to date"));
        let busy = memory(Some(Backfill {
            done: 40,
            total: 1_240,
            eta_secs: 3_725,
        }));
        let est = backfill_estimate(&busy);
        assert_eq!(
            (est.units, est.eta_secs, est.model.as_str()),
            (1_200, 3_725, "p/haiku")
        );
        let text = reply(MemoryMode::Endless, &busy);
        assert!(
            text.contains("1200 messages") && text.contains("1h 02m") && text.contains("p/haiku"),
            "{text}"
        );
        assert!(reply(MemoryMode::Epochs, &busy).starts_with("Endless memory off."));
        assert_eq!(eta(59), "59s");
        assert_eq!(eta(125), "2m 05s");
        let s = status(&busy);
        assert!(s.starts_with("Memory: endless · view 0 KB · 1 lines · 2 messages · compactor p/haiku · backfill 40/1240"), "{s}");
        assert!(status(&memory(None)).ends_with("· up to date"));
        let mut epochs = memory(None);
        epochs.mode = MemoryMode::Epochs;
        assert!(status(&epochs).starts_with("Memory: epochs"));
    }
}
