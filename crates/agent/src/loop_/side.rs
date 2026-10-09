//! One non-streaming, no-tools, no-history request on a role model (57 §2
//! `SideCall`): the detached path the title turn always used, generalised so
//! `decision::escalate` (tier 2) and the §11 composer run the same way — outside
//! the session's turn, fenced on the session's `closed` token, accounted under
//! their own [`CallKind`] against the session (51 §6.1).

use std::time::{Duration, Instant};

use futures::StreamExt;
use genai::chat::{ChatMessage, ChatStreamEvent, StreamEnd};
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
}

impl Shared {
    /// The role's model: `Escalate` → escalate → title → session; `Title` →
    /// title → session; `Vision` → the fallback or an error; `Session` → the
    /// session's model (its resolution error, if any); `Named` → the catalog.
    pub(crate) fn role_model(&self, role: &ModelRole) -> Result<Resolved, String> {
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
        match role {
            ModelRole::Escalate => slot(&self.escalate_model)
                .or_else(|| slot(&self.title_model))
                .map_or_else(session, Ok),
            ModelRole::Title => slot(&self.title_model).map_or_else(session, Ok),
            // 153-C stub — A5 fills: `polish_model → title_model → session` (153 §9.3).
            ModelRole::Polish => slot(&self.title_model).map_or_else(session, Ok),
            ModelRole::Vision => {
                slot(&self.vision_model).ok_or_else(|| "no vision fallback model is set".to_owned())
            }
            ModelRole::Session => session(),
            ModelRole::Named(id) => self
                .catalog
                .resolve(id, &*crate::supervisor::configured(self.auth.clone())),
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
        }
    }
}

impl Side {
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
        let current = Current::new(model, None, &self.client, &self.auth);
        let transport = current.backend.transport(&current.model).await?;
        let message = ChatMessage::user(user);
        // 116: the reply budget from the model's own limits; no calibration here.
        let (window, output) = current
            .model
            .model
            .as_ref()
            .map_or((0, 0), |m| (m.limit.context, m.limit.output));
        let est = compact::est(&message).saturating_add((system.len() / 4) as u32);
        let reply = compact::reply_budget(output, window, est, None, 0);
        let frame = Frame {
            tools: &[],
            system,
            head: None,
            history: std::slice::from_ref(&message),
            vision: false,
            strip_reasoning: false,
            max_tokens: (reply > 0).then_some(reply),
        };
        let (req, opts) = frame.request(transport, self.session, None);
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
