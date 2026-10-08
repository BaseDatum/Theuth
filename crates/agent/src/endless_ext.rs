//! 137 Revision 1 seams the loop, the spawn path, the roster and the RPC
//! call — thin delegations to [`Endless`] (W1 landed `render_snapshot`,
//! `render_with_budget`, `set_mode(…, UnitFilter)`, `set_view_bytes`,
//! `backfill_estimate` in `8f9cc634`), plus the `/endless` reply texts every
//! client shares.

use runtime::SessionId;

use crate::endless::{
    Backfill, Endless, MemoryMode, SessionMemory, SetMode, UnitFilter, ViewBlock, ViewLine,
};

/// What `/endless on` tells the user (R1-D3, R1-D4), from the service's
/// [`Backfill`] (`usd` is the service's remaining × measured $ per unit) and
/// the compactor model it resolved.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct BackfillEstimate {
    /// Units still to summarise.
    pub units: u64,
    /// Wall-clock estimate for the backfill.
    pub eta_secs: u64,
    /// Estimated cost in USD.
    pub usd: f64,
    /// The compactor model id (R1-D3 chain, resolved by the service).
    pub model: String,
}

/// R1-D1: the parent's rendered view as cache-marked blocks for a native
/// child's first message; empty when the parent has no view or the render fails.
pub async fn render_snapshot(endless: &Endless, session: SessionId) -> Vec<ViewBlock> {
    match endless.render_snapshot(session).await {
        Ok(blocks) => blocks,
        Err(e) => {
            tracing::warn!(session = %session, "endless render_snapshot: {e:#}");
            Vec::new()
        }
    }
}

/// `/endless on|off` with a unit filter (R1-D6).
pub async fn set_mode(
    endless: &Endless,
    session: SessionId,
    mode: MemoryMode,
    epoch: u32,
    filter: UnitFilter,
) -> anyhow::Result<SetMode> {
    endless.set_mode(session, mode, epoch, filter).await
}

/// R1-D6: the view re-folded to `bytes`, as lines (a meeting head's
/// section; the pinned shape of `agent::RenderMemoryView`).
pub async fn render_with_budget(
    endless: &Endless,
    session: SessionId,
    bytes: u32,
) -> anyhow::Result<Vec<ViewLine>> {
    endless.render_with_budget(session, bytes).await
}

/// R1-D3/R1-D4: the estimate `/endless on` replies with, from the state the
/// switch returned (`backfill: None` = the view already covers the history).
pub fn backfill_estimate(m: &SessionMemory) -> BackfillEstimate {
    let (units, eta_secs, usd) = match m.backfill {
        Some(Backfill {
            done,
            total,
            eta_secs,
            usd_estimate,
        }) => (total.saturating_sub(done), eta_secs, usd_estimate),
        None => (0, 0, 0.0),
    };
    BackfillEstimate {
        units,
        eta_secs,
        usd,
        model: m.model.clone(),
    }
}

/// R1-D2b: the per-session view budget (16_000..=256_000), applied by the
/// service at the next render; returns the state after the write.
pub async fn set_view_bytes(
    endless: &Endless,
    session: SessionId,
    bytes: u32,
) -> Result<SessionMemory, String> {
    if !(16_000..=256_000).contains(&bytes) {
        return Err(format!("view_bytes {bytes} out of range 16000..=256000"));
    }
    endless
        .set_view_bytes(session, bytes)
        .await
        .map_err(|e| format!("{e:#}"))?;
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
                format!(
                    "Endless memory on. Summarising {} messages in the background with {model} (ETA {}, ≈ ${:.2}); the session keeps its current memory until the view covers its history.",
                    est.units,
                    eta(est.eta_secs),
                    est.usd
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
        "Memory: endless · view {} KB of {} KB · {} lines · {} messages · compactor {}",
        bytes / 1000,
        m.view_bytes / 1000,
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
                at: None,
            }],
            settling: false,
            unbuilt: 0,
            units: 2,
            stats: EndlessStats::default(),
            model: "p/haiku".into(),
            view_bytes: 128_000,
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
            usd_estimate: 18.5,
        }));
        let est = backfill_estimate(&busy);
        assert_eq!(
            (est.units, est.eta_secs, est.model.as_str(), est.usd),
            (1_200, 3_725, "p/haiku", 18.5)
        );
        let text = reply(MemoryMode::Endless, &busy);
        assert!(
            text.contains("1200 messages")
                && text.contains("1h 02m")
                && text.contains("p/haiku")
                && text.contains("$18.50"),
            "{text}"
        );
        assert!(reply(MemoryMode::Epochs, &busy).starts_with("Endless memory off."));
        assert_eq!(eta(59), "59s");
        assert_eq!(eta(125), "2m 05s");
        let s = status(&busy);
        assert!(
            s.starts_with("Memory: endless · view 0 KB of 128 KB · 1 lines · 2 messages · compactor p/haiku · backfill 40/1240"),
            "{s}"
        );
        assert!(status(&memory(None)).ends_with("· up to date"));
        let mut epochs = memory(None);
        epochs.mode = MemoryMode::Epochs;
        assert!(status(&epochs).starts_with("Memory: epochs"));
    }
}
