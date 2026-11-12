//! One non-streaming, no-tools, no-history request on a role model (57 §2
//! `SideCall`): the detached path the title turn always used, generalised so
//! `decision::escalate` (tier 2) and the §11 composer run the same way — outside
//! the session's turn, fenced on the session's `closed` token, accounted under
//! their own [`CallKind`] against the session (51 §6.1).

use std::time::{Duration, Instant};

use futures::StreamExt;
use genai::chat::{ChatMessage, ChatStreamEvent, ReasoningEffort, StreamEnd};
use models::Resolved;
use uuid::Uuid;

use super::{Current, Frame, Reserved, Shared};
use crate::cost::{CallKind, CallStatus};
use crate::{ModelRole, SideReply, compact};

/// What a detached call needs from the session, cloned out of [`Shared`] so the
/// task owns it.
#[derive(Clone)]
pub(crate) struct Side {
    pub session: runtime::SessionId,
    pub turn: Option<runtime::TurnId>,
    pub store: crate::store::Store,
    pub rt: runtime::Runtime,
    pub closed: tokio_util::sync::CancellationToken,
    /// 136 §2.2: the session's money cap at the moment the call was asked for.
    pub budget: Option<crate::Budget>,
    client: genai::Client,
    auth: crate::supervisor::Auth,
    /// 177 R2-6: a title call (thinking off on a local server).
    short: bool,
    /// 194 D4e: the reasoning effort the call carries ([`Side::effort`]); `None` = no setting.
    effort: Option<ReasoningEffort>,
}

impl Shared {
    /// The role's model and the effort of the setting that resolved it (194
    /// D4e): `Escalate` → escalate → title → session; `Title` → title →
    /// session; `Polish` → polish → title → session (153 §9.3); `Vision` → the
    /// fallback or an error; `Session` → the session's model (its resolution
    /// error, if any); `Named`/`NamedWithEffort` → the catalog. The effort is
    /// the answering setting's own; a session-model fallback carries none.
    pub(crate) fn role_model(
        &self,
        role: &ModelRole,
    ) -> Result<(Resolved, Option<String>), String> {
        let slot = |s: &std::sync::RwLock<Option<Resolved>>| {
            s.read().unwrap_or_else(|p| p.into_inner()).clone()
        };
        let session = || {
            let current = self.model.lock().unwrap_or_else(|p| p.into_inner());
            match &current.error {
                Some(e) => Err(format!("model: {e}")),
                None => Ok(current.model.clone()),
            }
        };
        let named = |id: &str| {
            self.catalog
                .resolve(id, &*crate::supervisor::configured(self.auth.clone()))
        };
        // 194 S1(b): the Title/Escalate/Vision/Polish efforts are filled in here.
        match role {
            ModelRole::Escalate => slot(&self.escalate_model)
                .or_else(|| slot(&self.title_model))
                .map_or_else(session, Ok)
                .map(|m| (m, None)),
            ModelRole::Title => slot(&self.title_model)
                .map_or_else(session, Ok)
                .map(|m| (m, None)),
            ModelRole::Polish => slot(&self.polish_model)
                .or_else(|| slot(&self.title_model))
                .map_or_else(session, Ok)
                .map(|m| (m, None)),
            ModelRole::Vision => slot(&self.vision_model)
                .map(|m| (m, None))
                .ok_or_else(|| "no vision fallback model is set".to_owned()),
            ModelRole::Session => session().map(|m| (m, None)),
            ModelRole::Named(id) => named(id).map(|m| (m, None)),
            ModelRole::NamedWithEffort { model, effort } => {
                named(model).map(|m| (m, effort.clone()))
            }
        }
    }

    /// Snapshot for a detached call attributed to `turn` (the running turn, or `None`).
    pub(crate) fn side(&self, turn: Option<runtime::TurnId>) -> Side {
        Side {
            session: self.id,
            turn,
            store: self.store.clone(),
            rt: self.rt.clone(),
            closed: self.closed.clone(),
            budget: self.budget.read().clone(),
            client: self.client.clone(),
            auth: self.auth.clone(),
            short: false,
            effort: None,
        }
    }
}

impl Side {
    /// 177 R2-6: mark the call a title — a short summary-style call that gets
    /// the thinking-off template kwargs on a local server.
    pub(crate) fn short(mut self, short: bool) -> Self {
        self.short = short;
        self
    }

    /// 194 D4e: carry `effort` (a keyword or token budget) on `model`'s call.
    /// One the model does not take — or no known word — is dropped with a
    /// warning, so a stale setting never 400s the call; `None` sends no setting.
    pub(crate) fn effort(mut self, model: &Resolved, effort: Option<&str>) -> Self {
        self.effort = effort.and_then(|e| {
            if let Some(Err(why)) = model.model.as_ref().map(|m| m.validate_effort(e)) {
                tracing::warn!(model = %model.id, "side-call effort `{e}` dropped: {why}");
                return None;
            }
            let out = crate::effort_of(e);
            if out.is_none() && e != "default" {
                tracing::warn!(model = %model.id, "side-call effort `{e}` dropped: unknown effort");
            }
            out
        });
        self
    }

    /// One request: `system` + one user message, no tools, drained to the
    /// stream's `End`, reserved and settled as `kind` for this session. `Err`
    /// is the provider's message, `timed out` after `budget`, or `cancelled`
    /// when the session closed; the row is settled either way.
    pub(crate) async fn call(
        &self,
        model: Resolved,
        system: &str,
        user: String,
        budget: Duration,
        kind: CallKind,
    ) -> Result<SideReply, String> {
        let mut current = Current::new(model, None, &self.client, &self.auth);
        current.effort = self.effort.clone();
        let transport = current.backend.transport(&current.model).await?;
        let message = ChatMessage::user(user);
        // 116: the reply budget from the model's own limits; no calibration here.
        let (window, output) = current
            .model
            .model
            .as_ref()
            .map_or((0, 0), |m| (m.limit.context, m.limit.output));
        let est = compact::est(&message).saturating_add((system.len() / 4) as u32);
        // 116: an effort on Anthropic floors the reply above its thinking budget.
        let floor = if self.effort.is_some() {
            current.reply_floor(transport)
        } else {
            0
        };
        let reply = compact::reply_budget(output, window, est, None, floor);
        let frame = Frame {
            tools: &[],
            system,
            head: None,
            history: std::slice::from_ref(&message),
            vision: false,
            strip_reasoning: false,
            max_tokens: (reply > 0).then_some(reply),
        };
        let (req, opts) = frame.request(transport, self.session, self.effort.clone());
        // 177 E2 / R2-6: a title (a short summary-style call) to a local
        // OpenAI-protocol server thinks not — unless its setting picked an
        // effort (194); consult, advisor, notebook insights, ask, polish and
        // escalate keep the model's thinking.
        let opts = if self.short
            && self.effort.is_none()
            && current.backend.local_openai(&current.model)
        {
            crate::backend::thinking_off(opts)
        } else {
            opts
        };
        let request_id = Uuid::now_v7().to_string();
        // 136 §2.2: a detached call under a budgeted owner is refused like a
        // round call (`budget exhausted: …`); its cost counts either way.
        let reserved = Reserved::open(
            &self.store,
            &self.rt,
            self.session,
            &request_id,
            0,
            self.turn,
            kind,
            &current.model,
            self.budget.as_ref(),
        )
        .await
        .map_err(|e| e.to_string())?;
        let started = Instant::now();
        let call = async {
            let mut stream = current
                .backend
                .stream(&current.model, req, &opts, self.session)
                .await?;
            while let Some(ev) = stream.next().await {
                if let ChatStreamEvent::End(end) = ev? {
                    return Ok(end);
                }
            }
            Err::<StreamEnd, String>("stream ended without End event".into())
        };
        let outcome = tokio::select! {
            _ = self.closed.cancelled() => Err((CallStatus::Cancelled, "cancelled".to_owned())),
            r = tokio::time::timeout(budget, call) => match r {
                Ok(Ok(end)) => Ok(end),
                Ok(Err(e)) => Err((CallStatus::Failed, e)),
                Err(_) => Err((CallStatus::Failed, "timed out".to_owned())),
            },
        };
        let latency = started.elapsed();
        let (status, usage) = match &outcome {
            Ok(end) => (CallStatus::Settled, end.captured_usage.as_ref()),
            Err((status, _)) => (*status, None),
        };
        let priced = reserved.settle(status, usage, &current.model).await;
        let end = outcome.map_err(|(_, e)| e)?;
        let tokens = |n: Option<i32>| n.and_then(|n| u32::try_from(n).ok());
        Ok(SideReply {
            text: end.captured_texts().map(|t| t.concat()).unwrap_or_default(),
            model: current.model.id.clone(),
            input_tokens: tokens(end.captured_usage.as_ref().and_then(|u| u.prompt_tokens)),
            output_tokens: tokens(
                end.captured_usage
                    .as_ref()
                    .and_then(|u| u.completion_tokens),
            ),
            latency,
            cost_nanos: priced.filter(|m| m.currency == "USD").map(|m| m.nanos),
            request_id: Some(request_id),
        })
    }
}
