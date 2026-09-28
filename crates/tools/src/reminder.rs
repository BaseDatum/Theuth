//! Reminders (14 as-built 2026-09-23): `remind_me_in` / `cancel_reminder`.
//!
//! A reminder is a process-less job in the [`Jobs`] actor: an admitted
//! operation (`Kind::Reminder`) from creation until it fires or is cancelled,
//! so a pending one blocks the todo nudge exactly like a running job (47
//! §3.2). When the timer fires the guard settles with [`Settle::Reminder`]
//! (the model's only `<reminder>` path); a cancel retires it bare. The
//! [`ReminderEvent`]s on the bus are observation for the agent's store
//! (insert on set, delete on cancel; a fired one goes with its settlement),
//! and [`RearmReminder`] restores stored rows after a restart.

use std::time::Duration;

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use kameo::reply::DelegatedReply;
use runtime::SessionId;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio::task::AbortHandle;

use crate::activity::{Activity, Kind, OperationId};
use crate::jobs::{Jobs, admit_job};
use crate::{Risk, Tool, ToolCx, ToolOutput};

/// Pending reminders per session.
pub const MAX_PENDING: usize = 8;
/// Shortest delay `remind_me_in` accepts.
pub const MIN_SECS: u64 = 5;
/// Longest `message`.
pub const MAX_MESSAGE_CHARS: usize = 2000;

/// Bus events, observation only (the model is woken through the guard).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum ReminderEvent {
    /// A reminder was armed by the model (not re-armed from the store).
    #[serde(rename = "ReminderSet")]
    Set {
        /// Owning session.
        session: SessionId,
        /// `r<n>`.
        id: String,
        /// What comes back.
        message: String,
        /// Unix seconds.
        set_at: i64,
        /// Unix seconds.
        fires_at: i64,
        /// Requested delay.
        seconds: u64,
    },
    /// Retired without delivery (`cancel_reminder` or the session closed).
    #[serde(rename = "ReminderCancelled")]
    Cancelled {
        /// Owning session.
        session: SessionId,
        /// `r<n>`.
        id: String,
    },
}

/// `remind_me_in`'s reply: the tool's `structured` dict.
#[derive(Clone, Debug, Serialize, kameo::Reply)]
pub struct ReminderView {
    /// `r<n>`.
    pub id: String,
    /// RFC 3339, local offset.
    pub fires_at: String,
    /// Requested delay.
    pub seconds: u64,
}

/// `ask` → `Result<ReminderView, String>`: arm a timer for `session`.
/// Err names the cap and the pending ids when [`MAX_PENDING`] are pending.
pub struct SetReminder {
    /// Owning session.
    pub session: SessionId,
    /// What comes back as `<reminder>`.
    pub message: String,
    /// Delay; range-checked by the tool, not here.
    pub seconds: u64,
    /// [`crate::activity::current`] at the caller, as [`crate::jobs::Start`].
    pub activity: Option<Activity>,
}

/// `ask` → `Result<(), String>`: retire a pending reminder, no delivery.
#[derive(Clone, Debug)]
pub struct CancelReminder {
    /// Owning session.
    pub session: SessionId,
    /// `r<n>`.
    pub id: String,
}

/// `tell`: restore a stored row after a restart. Fires immediately with
/// `late="true"` when `fires_at` has passed; publishes no `ReminderSet`.
#[derive(Clone, Debug)]
pub struct RearmReminder {
    /// Owning session.
    pub session: SessionId,
    /// The stored `r<n>`; later ids of the session start above it.
    pub id: String,
    /// What comes back.
    pub message: String,
    /// Unix seconds.
    pub set_at: i64,
    /// Unix seconds.
    pub fires_at: i64,
    /// Requested delay.
    pub seconds: u64,
}

/// Second hop after admission (as `Launch`): spawn the timer under the actor.
struct Arm {
    session: SessionId,
    id: String,
    message: String,
    set_at: i64,
    fires_at: i64,
    seconds: u64,
    activity: Activity,
    /// Set by the model now (publish `ReminderSet`), not restored.
    fresh: bool,
}

/// `tell` from the timer task.
struct Fired {
    session: SessionId,
    id: String,
}

pub(crate) struct Reminder {
    id: String,
    op: OperationId,
    message: String,
    set_at: i64,
    seconds: u64,
    /// Re-armed past due: fires at once, `late="true"`.
    late: bool,
    task: AbortHandle,
}

#[derive(Default)]
pub(crate) struct SessionReminders {
    next: u32,
    pending: Vec<Reminder>,
}

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// RFC 3339 in the local offset.
pub fn local_rfc3339(unix: i64) -> String {
    chrono::DateTime::<chrono::Utc>::from_timestamp(unix, 0).map_or_else(String::new, |t| {
        t.with_timezone(&chrono::Local).to_rfc3339()
    })
}

impl Jobs {
    /// Admit off the mailbox, then [`Arm`] under the actor.
    fn admit_and_arm(
        &self,
        me: ActorRef<Jobs>,
        parent: Option<Activity>,
        arm: impl FnOnce(Activity) -> Arm + Send + 'static,
        reply: impl FnOnce(Result<ReminderView, String>) + Send + 'static,
        session: SessionId,
        id: String,
    ) {
        let sink = self.activity.clone();
        tokio::spawn(async move {
            let activity = match admit_job(parent, sink.as_ref(), session, Kind::Reminder(id)).await
            {
                Ok(a) => a,
                Err(why) => return reply(Err(why.to_string())),
            };
            reply(match me.ask(arm(activity)).await {
                Ok(v) => Ok(v),
                Err(e) => Err(e.to_string()),
            });
        });
    }

    /// Retire every pending reminder of a closing session (no delivery).
    pub(crate) async fn close_reminders(&mut self, session: SessionId) {
        let Some(sr) = self.reminders.remove(&session) else {
            return;
        };
        for r in sr.pending {
            r.task.abort();
            self.live.remove(&r.op);
            self.rt
                .publish(ReminderEvent::Cancelled { session, id: r.id })
                .await;
        }
    }
}

impl Message<SetReminder> for Jobs {
    type Reply = DelegatedReply<Result<ReminderView, String>>;

    async fn handle(
        &mut self,
        m: SetReminder,
        ctx: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let (reply, tx) = ctx.reply_sender();
        let Some(tx) = tx else { return reply };
        let sr = self.reminders.entry(m.session).or_default();
        // ponytail: checked before admission; two concurrent sets can both
        // pass. Re-check in `Arm` if that ever matters.
        if sr.pending.len() >= MAX_PENDING {
            let ids: Vec<&str> = sr.pending.iter().map(|r| r.id.as_str()).collect();
            tx.send(Err(format!(
                "{MAX_PENDING} reminders already pending ({}); cancel one first",
                ids.join(", ")
            )));
            return reply;
        }
        sr.next += 1;
        let id = format!("r{}", sr.next);
        let set_at = now();
        let (session, seconds, message) = (m.session, m.seconds, m.message);
        let arm = {
            let id = id.clone();
            move |activity| Arm {
                session,
                id,
                message,
                set_at,
                fires_at: set_at + seconds as i64,
                seconds,
                activity,
                fresh: true,
            }
        };
        self.admit_and_arm(
            ctx.actor_ref().clone(),
            m.activity,
            arm,
            move |r| tx.send(r),
            session,
            id,
        );
        reply
    }
}

impl Message<RearmReminder> for Jobs {
    type Reply = ();

    async fn handle(&mut self, m: RearmReminder, ctx: &mut Context<Self, Self::Reply>) {
        let sr = self.reminders.entry(m.session).or_default();
        if let Some(n) = m.id.strip_prefix('r').and_then(|n| n.parse().ok()) {
            sr.next = sr.next.max(n);
        }
        let (session, id) = (m.session, m.id.clone());
        let arm = move |activity| Arm {
            session,
            id: m.id,
            message: m.message,
            set_at: m.set_at,
            fires_at: m.fires_at,
            seconds: m.seconds,
            activity,
            fresh: false,
        };
        self.admit_and_arm(
            ctx.actor_ref().clone(),
            None,
            arm,
            |r| {
                if let Err(e) = r {
                    tracing::warn!("reminder not re-armed: {e}");
                }
            },
            session,
            id,
        );
    }
}

impl Message<Arm> for Jobs {
    type Reply = ReminderView;

    async fn handle(&mut self, m: Arm, ctx: &mut Context<Self, Self::Reply>) -> Self::Reply {
        let late = m.fires_at <= now();
        let delay = Duration::from_secs(u64::try_from(m.fires_at - now()).unwrap_or(0));
        self.track(&m.activity);
        let op = m.activity.id();
        let me = ctx.actor_ref().clone();
        let (session, id, activity) = (m.session, m.id.clone(), m.activity);
        let task = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = me.tell(Fired { session, id }).await;
            // The task's clone outlives the tell: the actor's settles it.
            drop(activity);
        })
        .abort_handle();
        self.reminders
            .entry(m.session)
            .or_default()
            .pending
            .push(Reminder {
                id: m.id.clone(),
                op,
                message: m.message.clone(),
                set_at: m.set_at,
                seconds: m.seconds,
                late,
                task,
            });
        if m.fresh {
            self.rt
                .publish(ReminderEvent::Set {
                    session: m.session,
                    id: m.id.clone(),
                    message: m.message,
                    set_at: m.set_at,
                    fires_at: m.fires_at,
                    seconds: m.seconds,
                })
                .await;
        }
        ReminderView {
            id: m.id,
            fires_at: local_rfc3339(m.fires_at),
            seconds: m.seconds,
        }
    }
}

impl Message<Fired> for Jobs {
    type Reply = ();

    async fn handle(&mut self, m: Fired, _: &mut Context<Self, Self::Reply>) {
        let Some(sr) = self.reminders.get_mut(&m.session) else {
            return;
        };
        let Some(i) = sr.pending.iter().position(|r| r.id == m.id) else {
            return;
        };
        let r = sr.pending.remove(i);
        // The actor's clone carries the payload; whichever clone drops last
        // settles `Settle::Reminder` (47 §4.1).
        if let Some(activity) = self.live.remove(&r.op) {
            activity.set_reminder(r.id, r.message, r.set_at, r.seconds, r.late);
        }
    }
}

impl Message<CancelReminder> for Jobs {
    type Reply = Result<(), String>;

    async fn handle(
        &mut self,
        m: CancelReminder,
        _: &mut Context<Self, Self::Reply>,
    ) -> Self::Reply {
        let sr = self
            .reminders
            .get_mut(&m.session)
            .ok_or_else(|| format!("no such reminder {}", m.id))?;
        let i = sr
            .pending
            .iter()
            .position(|r| r.id == m.id)
            .ok_or_else(|| format!("no such reminder {}", m.id))?;
        let r = sr.pending.remove(i);
        r.task.abort();
        // Both clones go (the task's with the abort): bare `Finished`.
        self.live.remove(&r.op);
        self.rt
            .publish(ReminderEvent::Cancelled {
                session: m.session,
                id: r.id,
            })
            .await;
        Ok(())
    }
}

/// Arguments for [`RemindMeInTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct RemindMeInArgs {
    /// Delay in seconds, 5 to 86400.
    pub seconds: u64,
    /// What to tell yourself when it fires (≤ 2000 chars).
    pub message: String,
}

/// `remind_me_in`: arm a timer; the message returns as `<reminder>`.
pub struct RemindMeInTool {
    /// The actor holding the timers.
    pub jobs: ActorRef<Jobs>,
    /// [`crate::ToolsConfig::reminder_max_secs`].
    pub max_secs: u64,
}

#[async_trait::async_trait]
impl Tool for RemindMeInTool {
    const NAME: &'static str = "remind_me_in";
    const RISK: Risk = Risk::ReadOnly;
    const DIRECT: bool = true;
    const RETURNS: &'static str = "dict";
    type Args = RemindMeInArgs;

    fn description(&self) -> String {
        "Set a timer instead of sleeping: returns at once; when it fires, `message` comes back to you as a `<reminder>` input and starts (or joins) a turn. Use it whenever you are waiting for a process, a deploy, a person or a job — end your turn after setting it. Limits: 5 s to 24 h, 8 pending per session.".into()
    }

    async fn call(&self, args: RemindMeInArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        if !(MIN_SECS..=self.max_secs).contains(&args.seconds) {
            return Ok(ToolOutput::err(format!(
                "seconds must be between {MIN_SECS} and {}",
                self.max_secs
            )));
        }
        if args.message.chars().count() > MAX_MESSAGE_CHARS {
            return Ok(ToolOutput::err(format!(
                "message longer than {MAX_MESSAGE_CHARS} chars"
            )));
        }
        let view = match self
            .jobs
            .ask(SetReminder {
                session: cx.session,
                message: args.message,
                seconds: args.seconds,
                activity: crate::activity::current(),
            })
            .await
        {
            Ok(v) => v,
            Err(kameo::error::SendError::HandlerError(e)) => return Ok(ToolOutput::err(e)),
            Err(e) => anyhow::bail!("jobs actor: {e}"),
        };
        let mut out = ToolOutput::ok(format!(
            "{} set: fires at {} (in {} s); end your turn, the message comes back as <reminder id=\"{}\">",
            view.id, view.fires_at, view.seconds, view.id
        ));
        out.structured = Some(serde_json::json!({
            "id": view.id,
            "fires_at": view.fires_at,
            "seconds": view.seconds,
        }));
        Ok(out)
    }
}

/// Arguments for [`CancelReminderTool`].
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CancelReminderArgs {
    /// Reminder id from `remind_me_in`, e.g. `r3`.
    pub id: String,
}

/// `cancel_reminder`: retire a pending reminder; nothing is delivered.
pub struct CancelReminderTool(pub ActorRef<Jobs>);

#[async_trait::async_trait]
impl Tool for CancelReminderTool {
    const NAME: &'static str = "cancel_reminder";
    const RISK: Risk = Risk::ReadOnly;
    type Args = CancelReminderArgs;

    fn description(&self) -> String {
        "Cancel a pending reminder by id; no <reminder> will follow for it.".into()
    }

    async fn call(&self, args: CancelReminderArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        Ok(
            match self
                .0
                .ask(CancelReminder {
                    session: cx.session,
                    id: args.id.clone(),
                })
                .await
            {
                Ok(()) => ToolOutput::ok(format!("cancelled {}", args.id)),
                Err(kameo::error::SendError::HandlerError(e)) => ToolOutput::err(e),
                Err(e) => anyhow::bail!("jobs actor: {e}"),
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::activity::test_sink::{Seen, drained, spawn};
    use crate::jobs::InstallActivity;
    use crate::jobs::tests::fixture;

    async fn set(
        fx: &crate::jobs::tests::Fx,
        seconds: u64,
        msg: &str,
    ) -> Result<ReminderView, String> {
        fx.jobs
            .ask(SetReminder {
                session: fx.session,
                message: msg.into(),
                seconds,
                activity: None,
            })
            .await
            .map_err(|e| match e {
                kameo::error::SendError::HandlerError(e) => e,
                e => e.to_string(),
            })
    }

    #[tokio::test]
    async fn set_admits_an_op_and_fires_settle_reminder() {
        let fx = fixture(Duration::ZERO, Duration::from_secs(30)).await;
        let (ledger, sink, seen) = spawn(None);
        fx.jobs.ask(InstallActivity(sink)).await.unwrap();
        let v = set(&fx, 1, "build should be done").await.unwrap();
        assert_eq!((v.id.as_str(), v.seconds), ("r1", 1));
        assert!(v.fires_at.contains('T'));
        let op = match drained(&ledger, &seen).await.as_slice() {
            [Seen::Admitted(op)] => {
                assert_eq!(op.kind, Kind::Reminder("r1".into()));
                assert_eq!(op.owner, fx.session);
                op.id
            }
            other => panic!("{other:?}"),
        };
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert_eq!(
            drained(&ledger, &seen).await.last(),
            Some(&Seen::Reminder(
                op,
                "r1".into(),
                "build should be done".into(),
                false
            ))
        );
    }

    #[tokio::test]
    async fn cancel_retires_without_delivery() {
        let fx = fixture(Duration::ZERO, Duration::from_secs(30)).await;
        let (ledger, sink, seen) = spawn(None);
        fx.jobs.ask(InstallActivity(sink)).await.unwrap();
        let v = set(&fx, 1, "x").await.unwrap();
        fx.jobs
            .ask(CancelReminder {
                session: fx.session,
                id: v.id.clone(),
            })
            .await
            .unwrap();
        let err = fx
            .jobs
            .ask(CancelReminder {
                session: fx.session,
                id: v.id,
            })
            .await
            .unwrap_err();
        assert!(err.to_string().contains("no such reminder r1"), "{err}");
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let seen = drained(&ledger, &seen).await;
        assert!(
            matches!(seen.as_slice(), [Seen::Admitted(op), Seen::Finished(f)] if op.id == *f),
            "{seen:?}"
        );
    }

    #[tokio::test]
    async fn per_session_cap_names_limit_and_ids() {
        let fx = fixture(Duration::ZERO, Duration::from_secs(30)).await;
        for _ in 0..MAX_PENDING {
            set(&fx, 600, "x").await.unwrap();
        }
        let err = set(&fx, 600, "one too many").await.unwrap_err();
        assert!(err.contains("8 reminders already pending"), "{err}");
        assert!(err.contains("r1, r2") && err.contains("r8"), "{err}");
        // Another session is unaffected.
        let other = SessionId::new();
        let v = fx
            .jobs
            .ask(SetReminder {
                session: other,
                message: "y".into(),
                seconds: 600,
                activity: None,
            })
            .await
            .unwrap();
        assert_eq!(v.id, "r1");
    }

    #[tokio::test]
    async fn rearm_overdue_fires_late_and_bumps_ids() {
        let fx = fixture(Duration::ZERO, Duration::from_secs(30)).await;
        let (ledger, sink, seen) = spawn(None);
        fx.jobs.ask(InstallActivity(sink)).await.unwrap();
        fx.jobs
            .tell(RearmReminder {
                session: fx.session,
                id: "r4".into(),
                message: "from before".into(),
                set_at: now() - 100,
                fires_at: now() - 40,
                seconds: 60,
            })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let seen = drained(&ledger, &seen).await;
        assert!(
            matches!(seen.as_slice(), [Seen::Admitted(_), Seen::Reminder(_, id, msg, true)] if id == "r4" && msg == "from before"),
            "{seen:?}"
        );
        assert_eq!(set(&fx, 600, "next").await.unwrap().id, "r5");
    }
}
