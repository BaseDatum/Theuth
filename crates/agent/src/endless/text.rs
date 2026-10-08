//! The texts every `/endless` surface (TUI, desktop, channels) replies with
//! (137 §8.6, R1-D3, R1-D4), rendered from the [`SessionMemory`] the switch or
//! the query returned — one wording for every client.

use super::{MemoryMode, SessionMemory};

/// `Nm Ns` / `Nh Nm` for an ETA.
pub fn eta(secs: u64) -> String {
    match secs {
        0 => "under a minute".to_owned(),
        s if s < 60 => format!("{s}s"),
        s if s < 3600 => format!("{}m {:02}s", s / 60, s % 60),
        s => format!("{}h {:02}m", s / 3600, (s % 3600) / 60),
    }
}

/// The text `/endless on|off` replies with, from the state the switch returned.
pub fn reply(mode: MemoryMode, m: &SessionMemory) -> String {
    match mode {
        MemoryMode::Endless => {
            let model = if m.model.is_empty() {
                "the session's model"
            } else {
                m.model.as_str()
            };
            match m.backfill {
                Some(b) if b.total > b.done => format!(
                    "Endless memory on. Summarising {} messages in the background with {model} (ETA {}, ≈ ${:.2}); the session keeps its current memory until the view covers its history.",
                    b.total - b.done,
                    eta(b.eta_secs),
                    b.usd_estimate
                ),
                _ => format!("Endless memory on. Compactor: {model}. The view is up to date."),
            }
        }
        MemoryMode::Epochs => {
            "Endless memory off. The view was written as this epoch's summary; the session continues with epochs.".to_owned()
        }
    }
}

/// One line of memory state for `/endless` with no argument: mode, view
/// size, lines, settle state, backfill, cost.
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
    use crate::endless::{Backfill, EndlessStats, ViewLine};

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
    fn reply_and_status_follow_the_backfill_state() {
        assert!(reply(MemoryMode::Endless, &memory(None)).contains("up to date"));
        let busy = memory(Some(Backfill {
            done: 40,
            total: 1_240,
            eta_secs: 3_725,
            usd_estimate: 2.5,
        }));
        let text = reply(MemoryMode::Endless, &busy);
        assert!(
            text.contains("1200 messages")
                && text.contains("1h 02m")
                && text.contains("p/haiku")
                && text.contains("$2.50"),
            "{text}"
        );
        assert!(reply(MemoryMode::Epochs, &busy).starts_with("Endless memory off."));
        assert_eq!(eta(59), "59s");
        assert_eq!(eta(125), "2m 05s");
        let s = status(&busy);
        assert!(
            s.starts_with(
                "Memory: endless · view 0 KB · 1 lines · 2 messages · compactor p/haiku · backfill 40/1240"
            ),
            "{s}"
        );
        assert!(status(&memory(None)).ends_with("· up to date"));
        let mut epochs = memory(None);
        epochs.mode = MemoryMode::Epochs;
        assert!(status(&epochs).starts_with("Memory: epochs"));
    }
}
