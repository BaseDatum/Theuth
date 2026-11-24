//! `agent.auto_archive`: the sweep that archives inactive root chats
//! ([`crate::auto_archive`] holds the rule). [`AutoArchiveSweep`] snapshots
//! nothing itself: the Jobs counts are read off the mailbox, then
//! [`AutoArchiveRound`] lists the durable sessions, drops desks, pinned, remote,
//! archived and busy trees, and archives the rest through
//! [`Agents::archive_session`] — the path the sidebar's Archive action takes.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use runtime::SessionId;

use super::Agents;
use crate::auto_archive::{self, AutoArchiveConfig};
use crate::{AgentEvent, AutoArchiveSweep, SetAutoArchive};

/// How often the sweep runs.
pub(super) const SWEEP_EVERY: Duration = Duration::from_secs(3600);
/// The first sweep waits this long after startup, so interrupted turns resume
/// (and count as running) first.
pub(super) const FIRST_SWEEP: Duration = Duration::from_secs(120);
/// Longest the Jobs actor may take to answer; a round it does not answer in is skipped.
const JOBS_PROBE: Duration = Duration::from_secs(5);

/// `tell` (internal): a sweep's Jobs reading. `None` = Jobs did not answer, so
/// nothing is archived this round (a live job could not be ruled out).
struct AutoArchiveRound(Option<HashSet<SessionId>>);

impl Message<AutoArchiveSweep> for Agents {
    type Reply = ();

    async fn handle(&mut self, _: AutoArchiveSweep, ctx: &mut Context<Self, ()>) {
        if !self.cfg.auto_archive.enabled {
            return;
        }
        let me = ctx.actor_ref().clone();
        tokio::spawn(async move {
            let jobs = live_job_sessions().await;
            let _ = me.tell(AutoArchiveRound(jobs)).await;
        });
    }
}

/// Sessions holding a running job, a PTY or an armed reminder. No Jobs actor
/// runs none; one that does not answer in [`JOBS_PROBE`] yields `None`.
async fn live_job_sessions() -> Option<HashSet<SessionId>> {
    let Ok(Some(jobs)) = ActorRef::<tools::Jobs>::lookup(tools::JOBS_ACTOR) else {
        return Some(HashSet::new());
    };
    match tokio::time::timeout(JOBS_PROBE, jobs.ask(tools::jobs::LiveCounts)).await {
        Ok(Ok(rows)) => Some(
            rows.into_iter()
                .filter(|r| !(r.jobs.is_empty() && r.ptys.is_empty() && r.reminders.is_empty()))
                .map(|r| r.session)
                .collect(),
        ),
        _ => None,
    }
}

impl Message<AutoArchiveRound> for Agents {
    type Reply = ();

    async fn handle(&mut self, AutoArchiveRound(jobs): AutoArchiveRound, _: &mut Context<Self, ()>) {
        let Some(jobs) = jobs else {
            tracing::debug!("auto-archive: jobs did not answer; skipped this round");
            return;
        };
        self.auto_archive_round(&jobs).await;
    }
}

impl Agents {
    /// Archive every root chat [`auto_archive::eligible`] picks now; `jobs` = the
    /// sessions holding a live job, PTY or reminder. The archived roots, oldest first.
    async fn auto_archive_round(&mut self, jobs: &HashSet<SessionId>) -> Vec<SessionId> {
        let cfg = self.cfg.auto_archive;
        if !cfg.enabled {
            return Vec::new();
        }
        // Durable sessions only: a private tree closes with its owner (51 §4).
        let rows = match self.store.list_sessions().await {
            Ok(rows) => rows,
            Err(e) => {
                tracing::warn!("auto-archive: list sessions: {e:#}");
                return Vec::new();
            }
        };
        let now = chrono::Utc::now();
        let stale: Vec<(SessionId, Duration, Option<String>)> =
            auto_archive::candidates(&rows, |id| jobs.contains(&id) || self.busy(id))
                .into_iter()
                .filter_map(|c| {
                    let age = auto_archive::eligible(&cfg, &c, now)?;
                    let project = rows.iter().find(|r| r.id == c.root)?.project.clone();
                    Some((c.root, age, project))
                })
                .collect();
        let mut done = Vec::new();
        for (session, age, project) in stale {
            if let Err(e) = self.archive_session(session, true).await {
                tracing::warn!(session = %session, "auto-archive: {e}");
                continue;
            }
            tracing::info!(
                session = %session,
                age = %humantime::format_duration(Duration::from_secs(age.as_secs())),
                after = %auto_archive::format_after(cfg.after),
                "auto-archive: archived inactive chat"
            );
            // Open lists refetch on it (as after a move); the root's project is unchanged.
            self.rt
                .publish(AgentEvent::SessionMetadataChanged { session, project })
                .await;
            done.push(session);
        }
        done
    }
}

impl Agents {
    /// `id` runs a turn, waits on an approval or a reply, or has a queued or
    /// running child spawn.
    fn busy(&self, id: SessionId) -> bool {
        self.running.contains(&id)
            || self.approvals.contains_key(&id)
            || self.child_turns.contains_key(&id)
            || self.queued.contains_key(&id)
            || self.waits.values().any(|w| w.session == id)
    }
}

impl Message<SetAutoArchive> for Agents {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: SetAutoArchive,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Result<(), String> {
        let after = auto_archive::check_after(Duration::from_secs(m.after_secs))?;
        let next = AutoArchiveConfig {
            enabled: m.enabled,
            after,
        };
        Arc::make_mut(&mut self.cfg).auto_archive = next;
        let path = self.rt.data_dir.join("config.yaml");
        for (key, value) in [
            ("enabled", m.enabled.to_string()),
            ("after", auto_archive::format_after(after)),
        ] {
            if let Err(e) =
                runtime::write_nested_config_value(&path, "agent", "auto_archive", key, &value)
            {
                tracing::warn!("config.yaml: {e}");
            }
        }
        // Applies at once: a shorter wait archives what is now past it.
        if next.enabled {
            let me = ctx.actor_ref().clone();
            tokio::spawn(async move {
                let _ = me.tell(AutoArchiveSweep).await;
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::time::Duration;

    use runtime::SessionId;

    use crate::auto_archive::AutoArchiveConfig;

    /// The sweep archives an old root through the Archive action's path (its
    /// child follows the root), and leaves desks, pinned, running, job-holding,
    /// recent and recently-active-child trees alone; off, it archives nothing.
    #[tokio::test]
    async fn sweep_archives_only_idle_plain_roots() {
        let (_scratch, mut a) = super::super::tests::agents_with(false).await;
        let ids: Vec<SessionId> = (0..8).map(|_| SessionId::new()).collect();
        let [old, old_child, desk, pinned, running, jobbed, recent, busy_tree] = ids[..] else {
            unreachable!()
        };
        let busy_child = SessionId::new();
        for (id, parent) in [
            (old, None),
            (old_child, Some(old)),
            (desk, None),
            (pinned, None),
            (running, None),
            (jobbed, None),
            (recent, None),
            (busy_tree, None),
            (busy_child, Some(busy_tree)),
        ] {
            a.store.create_session(id, "m", parent, None, None).await.unwrap();
        }
        let month_ago = (chrono::Utc::now() - chrono::Duration::days(30)).to_rfc3339();
        for id in [old, old_child, desk, pinned, running, jobbed, busy_tree] {
            a.store
                .exec_for_test(&format!(
                    "UPDATE sessions SET updated_at = '{month_ago}' WHERE id = '{id}'"
                ))
                .await;
        }
        a.store.set_desk(desk, Some("writer")).await.unwrap();
        a.store.set_pinned(pinned, true).await.unwrap();
        a.running.insert(running);
        let jobs = HashSet::from([jobbed]);

        Arc::make_mut(&mut a.cfg).auto_archive = AutoArchiveConfig {
            enabled: false,
            ..AutoArchiveConfig::default()
        };
        assert!(a.auto_archive_round(&jobs).await.is_empty(), "off archives nothing");

        Arc::make_mut(&mut a.cfg).auto_archive = AutoArchiveConfig::default();
        assert_eq!(a.auto_archive_round(&jobs).await, vec![old]);
        let archived: HashSet<SessionId> = a
            .store
            .list_sessions()
            .await
            .unwrap()
            .into_iter()
            .filter(|s| s.archived)
            .map(|s| s.id)
            .collect();
        assert_eq!(archived, HashSet::from([old, old_child]));
        // Already archived: the next sweep leaves it be.
        assert!(a.auto_archive_round(&jobs).await.is_empty());

        // A shorter wait catches the recent chat once it is past it.
        Arc::make_mut(&mut a.cfg).auto_archive.after = Duration::from_secs(3600);
        let two_hours_ago = (chrono::Utc::now() - chrono::Duration::hours(2)).to_rfc3339();
        a.store
            .exec_for_test(&format!(
                "UPDATE sessions SET updated_at = '{two_hours_ago}' WHERE id = '{recent}'"
            ))
            .await;
        assert_eq!(a.auto_archive_round(&jobs).await, vec![recent]);
    }
}
