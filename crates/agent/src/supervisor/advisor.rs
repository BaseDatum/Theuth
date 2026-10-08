//! 125 §4.1 / §6.3: the supervisor side of the advisor — the `ask` targets of
//! `set_advisor`, `advisor_status` and the `set_advisor_*` defaults, plus the
//! `tell`s the `Advisors` actor sends back (routing an update into the
//! advisor session, handing a card to the advised session's actor).

use kameo::actor::ActorRef;
use kameo::message::{Context, Message};
use tools::{REGISTRY_ACTOR, Risk, SetApprovals, ToolRegistry};

use super::Agents;
use crate::advisor::{
    ADVISORS_ACTOR, AdvisorInput, Advisors, Attach, DeliverAdvisory, Detach, StatusOf,
};
use crate::continuation::InputOrigin;
use crate::loop_::{SetToolsetOverride, SteerAdvisory};
use crate::{
    AdvisorDefaults, AdvisorLink, AdvisorState, AdvisorStatus, AdvisorStatusOf, AppendAdvisory,
    GetAdvisorDefaults, SessionKind, SetAdvisor, SetAdvisorDefaults, SetAdvisorOn, SetEffort,
    SetModel, SyncBacklog, Toolset,
};

/// D2: the advisor's fixed read-only direct toolset — no runtime, no hints.
const ADVISOR_TOOLS: [&str; 10] = [
    "read_file",
    "grep",
    "glob",
    "list_dir",
    "code_find",
    "code_outline",
    "code_symbol",
    "code_refs",
    "knowledge_search",
    "knowledge_get",
];

fn advisor_toolset() -> Toolset {
    Toolset {
        tools: ADVISOR_TOOLS.iter().map(|s| (*s).to_owned()).collect(),
        prompts: Vec::new(),
        runtime: false,
        hint_limit: Some(0),
    }
}

fn advisors() -> Result<ActorRef<Advisors>, String> {
    ActorRef::<Advisors>::lookup(ADVISORS_ACTOR)
        .ok()
        .flatten()
        .ok_or_else(|| "advisor actor not running".to_owned())
}

fn sync_backlog_str(v: SyncBacklog) -> &'static str {
    match v {
        SyncBacklog::Off => "off",
        SyncBacklog::One => "\"1\"",
        SyncBacklog::Three => "\"3\"",
        SyncBacklog::Five => "\"5\"",
    }
}

/// D-R1.5: one crude pairing signal — both catalog prices known and the
/// advisor's output price below the advised model's.
fn pairing_warning(advisor: Option<models::Cost>, advised: Option<models::Cost>) -> Option<String> {
    let (a, s) = (advisor?, advised?);
    (a.output < s.output).then(|| PAIRING_WARNING.to_owned())
}

/// The D-R1.5 warning text.
pub const PAIRING_WARNING: &str =
    "advisor model prices below the session model (crude signal)";

impl Agents {
    fn write_advisor_key(&self, key: &str, value: &str) {
        let path = self.rt.data_dir.join("config.yaml");
        if let Err(e) = runtime::write_config_value(&path, "advisor", key, value) {
            tracing::warn!("config.yaml: {e}");
        }
    }

    /// 125 §4.1 items 1–3: turn the advisor of `session` on with `model`.
    async fn advisor_on(
        &mut self,
        session: runtime::SessionId,
        model: String,
        effort: Option<String>,
    ) -> Result<AdvisorStatus, String> {
        let row = self
            .store
            .session(session)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("session {session} not found"))?;
        if row.kind != SessionKind::Chat {
            return Err("an advisor cannot be advised".into());
        }
        if row.agent.is_some() {
            return Err(super::NOT_NATIVE.into());
        }
        let resolved = self.catalog.resolve(&model, &*self.configured)?;
        let existing = self
            .store
            .advisor_link(session)
            .await
            .map_err(|e| e.to_string())?;
        let advisor = match existing {
            Some(link) => {
                // Item 2: re-pick keeps the advisor's context; `manual: false`
                // so the agent default never moves (82 §2).
                self.set_model(SetModel {
                    session: Some(link.advisor),
                    model: resolved.id.clone(),
                    manual: false,
                })
                .await?;
                link.advisor
            }
            None => {
                // Item 1: the hidden child, created with its kind, then given
                // the read_only cap, no approval prompts and the fixed toolset.
                let id = runtime::SessionId::new();
                self.store
                    .create_session_kind(
                        id,
                        &resolved.id,
                        Some(session),
                        Some("advisor"),
                        None,
                        SessionKind::Advisor,
                    )
                    .await
                    .map_err(|e| e.to_string())?;
                let (_, actor) = self
                    .session(Some(id), None, None, (None, None))
                    .await
                    .map_err(|e| format!("{e:#}"))?;
                self.install_cap(id, Risk::ReadOnly).await;
                if let Ok(Some(reg)) = ActorRef::<ToolRegistry>::lookup(REGISTRY_ACTOR) {
                    let _ = reg
                        .tell(SetApprovals {
                            session: Some(id),
                            enabled: false,
                        })
                        .await;
                }
                let _ = actor
                    .native()?
                    .tell(SetToolsetOverride(Some(advisor_toolset())))
                    .await;
                id
            }
        };
        self.set_effort(SetEffort {
            session: Some(advisor),
            effort: effort.clone(),
            manual: false,
        })
        .await?;
        // Item 3: the pick pins the picker and serves `/advisor on` (D1).
        self.advisor_cfg.model = Some(resolved.id.clone());
        self.advisor_cfg.effort = effort.clone();
        self.write_advisor_key("model", &resolved.id);
        self.write_advisor_key(
            "effort",
            &effort
                .as_deref()
                .map_or("~".to_owned(), |e| format!("\"{e}\"")),
        );
        let context_limit = resolved.model.as_ref().map_or(0, |m| m.limit.context);
        let warning = pairing_warning(
            resolved.model.as_ref().and_then(|m| m.cost),
            self.catalog
                .resolve(&row.model, &*self.configured)
                .ok()
                .and_then(|r| r.model.and_then(|m| m.cost)),
        );
        let status = advisors()?
            .ask(Attach {
                link: AdvisorLink {
                    session,
                    advisor,
                    model: resolved.id,
                    effort,
                    cursor: 0,
                    state: AdvisorState::Idle,
                },
                context_limit,
                warning,
            })
            .await
            .map_err(|e| e.to_string())?;
        // Item 4: the advised loop's hooks switch on.
        if let Some(native) = self.live(session).and_then(|a| a.native().ok()) {
            let _ = native.tell(SetAdvisorOn(true)).await;
        }
        Ok(status)
    }

    /// `set_advisor(session, None)`: dispose (125 §4.1).
    async fn advisor_off(&mut self, session: runtime::SessionId) -> Result<AdvisorStatus, String> {
        let off = AdvisorStatus {
            session,
            ..AdvisorStatus::default()
        };
        if self
            .store
            .advisor_link(session)
            .await
            .map_err(|e| e.to_string())?
            .is_none()
        {
            return Ok(off);
        }
        advisors()?
            .ask(Detach(session))
            .await
            .map_err(|e| e.to_string())?;
        if let Some(native) = self.live(session).and_then(|a| a.native().ok()) {
            let _ = native.tell(SetAdvisorOn(false)).await;
        }
        Ok(off)
    }
}

impl Message<SetAdvisor> for Agents {
    type Reply = Result<AdvisorStatus, String>;

    async fn handle(
        &mut self,
        m: SetAdvisor,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<AdvisorStatus, String> {
        match m.model {
            Some(model) => self.advisor_on(m.session, model, m.effort).await,
            None => self.advisor_off(m.session).await,
        }
    }
}

impl Message<AdvisorStatusOf> for Agents {
    type Reply = AdvisorStatus;

    async fn handle(
        &mut self,
        m: AdvisorStatusOf,
        _: &mut Context<Self, Self::Reply>,
    ) -> AdvisorStatus {
        match advisors() {
            Ok(a) => a
                .ask(StatusOf(m.session))
                .await
                .unwrap_or_else(|_| AdvisorStatus {
                    session: m.session,
                    ..AdvisorStatus::default()
                }),
            Err(_) => AdvisorStatus {
                session: m.session,
                ..AdvisorStatus::default()
            },
        }
    }
}

impl Message<SetAdvisorDefaults> for Agents {
    type Reply = Result<AdvisorDefaults, String>;

    async fn handle(
        &mut self,
        m: SetAdvisorDefaults,
        _: &mut Context<Self, Self::Reply>,
    ) -> Result<AdvisorDefaults, String> {
        if let Some(n) = m.max_notes_per_update
            && !(1..=32).contains(&n)
        {
            return Err("max_notes_per_update must be 1–32".into());
        }
        if let Some(n) = m.immune_turns
            && n > 10
        {
            return Err("immune_turns must be 0–10".into());
        }
        if let Some(n) = m.max_consults
            && n > 32
        {
            return Err("max_consults must be 0–32".into());
        }
        if let Some(effort) = m.effort {
            self.write_advisor_key(
                "effort",
                &effort
                    .as_deref()
                    .map_or("~".to_owned(), |e| format!("\"{e}\"")),
            );
            self.advisor_cfg.effort = effort;
        }
        if let Some(n) = m.max_notes_per_update {
            self.write_advisor_key("max_notes_per_update", &n.to_string());
            self.advisor_cfg.max_notes_per_update = n;
        }
        if let Some(n) = m.immune_turns {
            self.write_advisor_key("immune_turns", &n.to_string());
            self.advisor_cfg.immune_turns = n;
        }
        if let Some(v) = m.sync_backlog {
            self.write_advisor_key("sync_backlog", sync_backlog_str(v));
            self.advisor_cfg.sync_backlog = v;
        }
        if let Some(n) = m.max_consults {
            self.write_advisor_key("max_consults", &n.to_string());
            self.advisor_cfg.max_consults = n;
        }
        if let Ok(a) = advisors() {
            let _ = a
                .tell(crate::advisor::SetDefaults(self.advisor_cfg.clone()))
                .await;
        }
        Ok(self.advisor_cfg.defaults())
    }
}

impl Message<GetAdvisorDefaults> for Agents {
    type Reply = AdvisorDefaults;

    async fn handle(
        &mut self,
        _: GetAdvisorDefaults,
        _: &mut Context<Self, Self::Reply>,
    ) -> AdvisorDefaults {
        self.advisor_cfg.defaults()
    }
}

/// 125 §4.5: one update → one advisor turn, routed exactly as a reminder is
/// (`inject`) minus the `MessageDelivered` event.
impl Message<AdvisorInput> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: AdvisorInput, _: &mut Context<Self, Self::Reply>) {
        let ticket = self.ledger.admit(m.advisor, InputOrigin::Automatic, None);
        if let Err(e) = self
            .route(m.advisor, None, m.text, Vec::new(), ticket, None, false)
            .await
        {
            tracing::warn!(session = %m.advisor, "advisor update: {e}");
        }
    }
}

/// 125 §4.10: a card for the advised session — a steer (its own turn when
/// idle) or a passive aside/preserve.
impl Message<DeliverAdvisory> for Agents {
    type Reply = ();

    async fn handle(&mut self, m: DeliverAdvisory, _: &mut Context<Self, Self::Reply>) {
        let actor = match self
            .session(Some(m.session), None, None, (None, None))
            .await
        {
            Ok((_, actor)) => actor,
            Err(e) => {
                tracing::warn!(session = %m.session, "advisory delivery: {e:#}");
                return;
            }
        };
        let Ok(native) = actor.native() else {
            return;
        };
        let r = if m.steer {
            let ticket = self.ledger.admit(m.session, InputOrigin::Automatic, None);
            native
                .tell(SteerAdvisory {
                    advisory: m.advisory,
                    text: m.text,
                    ticket: Some(ticket),
                })
                .await
                .map_err(|e| e.to_string())
        } else {
            native
                .tell(AppendAdvisory {
                    advisory: m.advisory,
                    text: m.text,
                    model_visible: m.model_visible,
                })
                .await
                .map_err(|e| e.to_string())
        };
        if let Err(e) = r {
            tracing::warn!(session = %m.session, "advisory delivery: {e}");
        }
    }
}
