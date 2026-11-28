//! JSON-RPC API and bounded per-Deck subscription for Decks (design doc 229).

#![allow(clippy::too_many_arguments)]

use std::sync::Arc;

use document::Principal;
use document::deck::{DeckDocError, DeckSection, SectionKind, SectionRevision};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, SubscriptionResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::server::{PendingSubscriptionSink, SubscriptionMessage};
use jsonrpsee::types::ErrorObjectOwned;
use kameo::actor::ActorRef;
use rpc_core::{NOT_RUNNING, Server, failed, lookup, peer};
use serde_json::Value;

use crate::hub::DeckHub;
use crate::model::{
    DeckDetail, DeckRuntimeEvent, DeckRuntimeStatus, DeckSnapshot, DeckSummary, DeclaredAction,
    InteractResponse, RuntimeControlOp, ScriptMode, ScriptRevision,
};
use crate::owner::{
    ApproveScriptMsg, CreateDeckMsg, DECKS_ACTOR, DeckSupervisor, FocusSectionMsg, GetDeck,
    GetSnapshotMsg, InteractMsg, ListDecks, ListSnapshotsMsg, ProposeScriptMsg, ReadScriptMsg,
    RejectScriptMsg, RuntimeControlMsg, SectionAddMsg, SectionArchiveMsg, SectionHistoryMsg,
    SectionMoveMsg, SectionPatchMsg, SubscribeDeckMsg,
};

fn decks_actor() -> RpcResult<ActorRef<DeckSupervisor>> {
    lookup::<DeckSupervisor>(DECKS_ACTOR).map_err(|e| match e.code() {
        NOT_RUNNING => ErrorObjectOwned::owned(NOT_RUNNING, "decks are disabled", None::<()>),
        _ => e,
    })
}

fn owner_principal(ext: &Extensions) -> RpcResult<(String, Principal)> {
    let p = peer(ext)?;
    let caller_id = format!("owner:{}", p.id());
    Ok((
        caller_id,
        Principal::Owner {
            device: Some(p.id()),
        },
    ))
}

fn map_doc_err(e: DeckDocError) -> ErrorObjectOwned {
    match e {
        DeckDocError::Conflict(c) => ErrorObjectOwned::owned(
            -32010,
            format!(
                "section_conflict: section {} is at rev {} (expected {})",
                c.section_id, c.actual_rev, c.expected_rev
            ),
            Some(c),
        ),
        other => failed(other.to_string()),
    }
}

fn map_send_err<M>(e: kameo::error::SendError<M, String>) -> ErrorObjectOwned {
    match e {
        kameo::error::SendError::HandlerError(msg) => failed(msg),
        other => failed(other.to_string()),
    }
}

fn map_doc_send_err<M>(e: kameo::error::SendError<M, DeckDocError>) -> ErrorObjectOwned {
    match e {
        kameo::error::SendError::HandlerError(doc_err) => map_doc_err(doc_err),
        other => failed(other.to_string()),
    }
}

struct UnsubGuard {
    hub: Arc<DeckHub>,
    deck_id: String,
    sub_id: u64,
}

impl Drop for UnsubGuard {
    fn drop(&mut self) {
        self.hub.unsubscribe(&self.deck_id, self.sub_id);
    }
}

/// Decks RPC methods owned by the `decks` plugin.
#[rpc_core_macros::rpc_descriptors(owner = "decks", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait DecksApi {
    /// List collaborative Decks with runtime health and revision summary.
    #[method(name = "deck_list")]
    async fn deck_list(&self, limit: Option<u32>) -> RpcResult<Vec<DeckSummary>>;

    /// Create a new collaborative Deck (or the built-in Workspace Pulse example when `example` is true).
    #[method(name = "deck_create", with_extensions)]
    async fn deck_create(
        &self,
        title: Option<String>,
        description: Option<String>,
        example: Option<bool>,
    ) -> RpcResult<DeckDetail>;

    /// Load a Deck's collaborative sections, runtime status, active/pending script revisions, and latest snapshot/frame.
    #[method(name = "deck_get", with_extensions)]
    async fn deck_get(&self, deck_id: String) -> RpcResult<DeckDetail>;

    /// Add a new stable-ID section (`json_render`, `note`, or `html`) to a Deck.
    #[method(name = "deck_section_add", with_extensions)]
    #[allow(clippy::too_many_arguments)]
    async fn deck_section_add(
        &self,
        deck_id: String,
        kind: SectionKind,
        title: String,
        content: String,
        col_span: Option<u32>,
        data_key: Option<String>,
        index: Option<usize>,
    ) -> RpcResult<DeckSection>;

    /// Patch one section in-place with optional optimistic CAS (`expected_rev`).
    #[method(name = "deck_section_patch", with_extensions)]
    #[allow(clippy::too_many_arguments)]
    async fn deck_section_patch(
        &self,
        deck_id: String,
        section_id: String,
        expected_rev: Option<u64>,
        title: Option<String>,
        content: Option<String>,
        col_span: Option<u32>,
        data_key: Option<Option<String>>,
        summary: Option<String>,
    ) -> RpcResult<DeckSection>;

    /// Move a section to a new display index using fractional rank ordering.
    #[method(name = "deck_section_move", with_extensions)]
    async fn deck_section_move(
        &self,
        deck_id: String,
        section_id: String,
        to_index: usize,
    ) -> RpcResult<Vec<String>>;

    /// Archive or unarchive a section (preserving revision history).
    #[method(name = "deck_section_archive", with_extensions)]
    async fn deck_section_archive(
        &self,
        deck_id: String,
        section_id: String,
        archived: bool,
        expected_rev: Option<u64>,
    ) -> RpcResult<DeckSection>;

    /// Read per-section revision history and attribution.
    #[method(name = "deck_section_history", with_extensions)]
    async fn deck_section_history(
        &self,
        deck_id: String,
        section_id: String,
    ) -> RpcResult<Vec<SectionRevision>>;

    /// Set or clear collaborative focus presence on a Deck section.
    #[method(name = "deck_focus", with_extensions)]
    async fn deck_focus(&self, deck_id: String, section_id: Option<String>) -> RpcResult<()>;

    /// Propose a new Monty script revision (always enters as `pending_review`).
    #[method(name = "deck_script_propose", with_extensions)]
    #[allow(clippy::too_many_arguments)]
    async fn deck_script_propose(
        &self,
        deck_id: String,
        code: String,
        declared_actions: Vec<DeclaredAction>,
        mode: ScriptMode,
        interval_secs: Option<u64>,
        schedule_cron: Option<String>,
        change_summary: Option<String>,
    ) -> RpcResult<ScriptRevision>;

    /// Owner approval gate: approve a `pending_review` script revision so it may run.
    #[method(name = "deck_script_approve", with_extensions)]
    async fn deck_script_approve(&self, deck_id: String, rev: u64) -> RpcResult<ScriptRevision>;

    /// Reject a `pending_review` script revision.
    #[method(name = "deck_script_reject", with_extensions)]
    async fn deck_script_reject(
        &self,
        deck_id: String,
        rev: u64,
        reason: Option<String>,
    ) -> RpcResult<ScriptRevision>;

    /// Read a Deck's script revision (specific `rev`, or pending/active/latest if omitted).
    #[method(name = "deck_script_read")]
    async fn deck_script_read(
        &self,
        deck_id: String,
        rev: Option<u64>,
    ) -> RpcResult<Option<ScriptRevision>>;

    /// Control a Deck's single runtime owner (`start`, `pause`, `resume`, `stop`, `run_once`).
    #[method(name = "deck_runtime_control")]
    async fn deck_runtime_control(
        &self,
        deck_id: String,
        op: RuntimeControlOp,
    ) -> RpcResult<DeckRuntimeStatus>;

    /// Invoke a declared interactive action on the Deck's live Monty checkout (returns private result to caller).
    #[method(name = "deck_interact", with_extensions)]
    async fn deck_interact(
        &self,
        deck_id: String,
        action: String,
        args: Value,
        request_id: String,
        expected_rev: Option<u64>,
        timeout_ms: Option<u64>,
    ) -> RpcResult<InteractResponse>;

    /// List recent durable snapshots persisted for a Deck.
    #[method(name = "deck_snapshots")]
    async fn deck_snapshots(
        &self,
        deck_id: String,
        limit: Option<u32>,
    ) -> RpcResult<Vec<DeckSnapshot>>;

    /// Fetch one pinned durable snapshot by ID (used by whiteboard cards with a pinned snapshot).
    #[method(name = "deck_snapshot_get")]
    async fn deck_snapshot_get(
        &self,
        deck_id: String,
        snapshot_id: String,
    ) -> RpcResult<Option<DeckSnapshot>>;

    /// Subscribe to bounded, coalesced per-Deck runtime events (sync, live frames, durable snapshots, status, approvals).
    #[subscription(name = "deck_subscribe", unsubscribe = "deck_unsubscribe", item = DeckRuntimeEvent)]
    async fn deck_subscribe(&self, deck_id: String) -> SubscriptionResult;
}

#[async_trait]
impl DecksApiServer for Server {
    async fn deck_list(&self, limit: Option<u32>) -> RpcResult<Vec<DeckSummary>> {
        let actor = decks_actor()?;
        actor
            .ask(ListDecks { limit })
            .send()
            .await
            .map_err(map_send_err)
    }

    async fn deck_create(
        &self,
        ext: &Extensions,
        title: Option<String>,
        description: Option<String>,
        example: Option<bool>,
    ) -> RpcResult<DeckDetail> {
        let (_, by) = owner_principal(ext)?;
        let actor = decks_actor()?;
        actor
            .ask(CreateDeckMsg {
                title,
                description,
                example: example.unwrap_or(false),
                by,
            })
            .send()
            .await
            .map_err(map_send_err)
    }

    async fn deck_get(&self, ext: &Extensions, deck_id: String) -> RpcResult<DeckDetail> {
        let (_, by) = owner_principal(ext)?;
        let actor = decks_actor()?;
        actor
            .ask(GetDeck { deck_id, by })
            .send()
            .await
            .map_err(map_send_err)
    }

    async fn deck_section_add(
        &self,
        ext: &Extensions,
        deck_id: String,
        kind: SectionKind,
        title: String,
        content: String,
        col_span: Option<u32>,
        data_key: Option<String>,
        index: Option<usize>,
    ) -> RpcResult<DeckSection> {
        let (_, by) = owner_principal(ext)?;
        let actor = decks_actor()?;
        actor
            .ask(SectionAddMsg {
                deck_id,
                kind,
                title,
                content,
                col_span,
                data_key,
                index,
                by,
            })
            .send()
            .await
            .map_err(map_doc_send_err)
    }

    async fn deck_section_patch(
        &self,
        ext: &Extensions,
        deck_id: String,
        section_id: String,
        expected_rev: Option<u64>,
        title: Option<String>,
        content: Option<String>,
        col_span: Option<u32>,
        data_key: Option<Option<String>>,
        summary: Option<String>,
    ) -> RpcResult<DeckSection> {
        let (_, by) = owner_principal(ext)?;
        let actor = decks_actor()?;
        actor
            .ask(SectionPatchMsg {
                deck_id,
                section_id,
                expected_rev,
                title,
                content,
                col_span,
                data_key,
                summary,
                by,
            })
            .send()
            .await
            .map_err(map_doc_send_err)
    }

    async fn deck_section_move(
        &self,
        ext: &Extensions,
        deck_id: String,
        section_id: String,
        to_index: usize,
    ) -> RpcResult<Vec<String>> {
        let (_, by) = owner_principal(ext)?;
        let actor = decks_actor()?;
        actor
            .ask(SectionMoveMsg {
                deck_id,
                section_id,
                to_index,
                by,
            })
            .send()
            .await
            .map_err(map_doc_send_err)
    }

    async fn deck_section_archive(
        &self,
        ext: &Extensions,
        deck_id: String,
        section_id: String,
        archived: bool,
        expected_rev: Option<u64>,
    ) -> RpcResult<DeckSection> {
        let (_, by) = owner_principal(ext)?;
        let actor = decks_actor()?;
        actor
            .ask(SectionArchiveMsg {
                deck_id,
                section_id,
                archived,
                expected_rev,
                by,
            })
            .send()
            .await
            .map_err(map_doc_send_err)
    }

    async fn deck_section_history(
        &self,
        ext: &Extensions,
        deck_id: String,
        section_id: String,
    ) -> RpcResult<Vec<SectionRevision>> {
        let (_, by) = owner_principal(ext)?;
        let actor = decks_actor()?;
        actor
            .ask(SectionHistoryMsg {
                deck_id,
                section_id,
                by,
            })
            .send()
            .await
            .map_err(map_doc_send_err)
    }

    async fn deck_focus(
        &self,
        ext: &Extensions,
        deck_id: String,
        section_id: Option<String>,
    ) -> RpcResult<()> {
        let (_, by) = owner_principal(ext)?;
        let actor = decks_actor()?;
        actor
            .ask(FocusSectionMsg {
                deck_id,
                section_id,
                by,
            })
            .send()
            .await
            .map_err(map_doc_send_err)
    }

    async fn deck_script_propose(
        &self,
        ext: &Extensions,
        deck_id: String,
        code: String,
        declared_actions: Vec<DeclaredAction>,
        mode: ScriptMode,
        interval_secs: Option<u64>,
        schedule_cron: Option<String>,
        change_summary: Option<String>,
    ) -> RpcResult<ScriptRevision> {
        let (caller_id, _) = owner_principal(ext)?;
        let actor = decks_actor()?;
        actor
            .ask(ProposeScriptMsg {
                deck_id,
                code,
                declared_actions,
                mode,
                interval_secs,
                schedule_cron,
                proposed_by: caller_id,
                change_summary: change_summary.unwrap_or_else(|| "proposed script revision".into()),
            })
            .send()
            .await
            .map_err(map_send_err)
    }

    async fn deck_script_approve(
        &self,
        ext: &Extensions,
        deck_id: String,
        rev: u64,
    ) -> RpcResult<ScriptRevision> {
        let (caller_id, _) = owner_principal(ext)?;
        let actor = decks_actor()?;
        actor
            .ask(ApproveScriptMsg {
                deck_id,
                rev,
                approved_by: caller_id,
            })
            .send()
            .await
            .map_err(map_send_err)
    }

    async fn deck_script_reject(
        &self,
        _ext: &Extensions,
        deck_id: String,
        rev: u64,
        reason: Option<String>,
    ) -> RpcResult<ScriptRevision> {
        let actor = decks_actor()?;
        actor
            .ask(RejectScriptMsg {
                deck_id,
                rev,
                reason,
            })
            .send()
            .await
            .map_err(map_send_err)
    }

    async fn deck_script_read(
        &self,
        deck_id: String,
        rev: Option<u64>,
    ) -> RpcResult<Option<ScriptRevision>> {
        let actor = decks_actor()?;
        actor
            .ask(ReadScriptMsg { deck_id, rev })
            .send()
            .await
            .map_err(map_send_err)
    }

    async fn deck_runtime_control(
        &self,
        deck_id: String,
        op: RuntimeControlOp,
    ) -> RpcResult<DeckRuntimeStatus> {
        let actor = decks_actor()?;
        actor
            .ask(RuntimeControlMsg { deck_id, op })
            .send()
            .await
            .map_err(map_send_err)
    }

    async fn deck_interact(
        &self,
        ext: &Extensions,
        deck_id: String,
        action: String,
        args: Value,
        request_id: String,
        expected_rev: Option<u64>,
        timeout_ms: Option<u64>,
    ) -> RpcResult<InteractResponse> {
        let (caller_id, _) = owner_principal(ext)?;
        let actor = decks_actor()?;
        actor
            .ask(InteractMsg {
                deck_id,
                request_id,
                caller_id,
                action,
                args,
                expected_rev,
                timeout_ms,
            })
            .send()
            .await
            .map_err(map_send_err)
    }

    async fn deck_snapshots(
        &self,
        deck_id: String,
        limit: Option<u32>,
    ) -> RpcResult<Vec<DeckSnapshot>> {
        let actor = decks_actor()?;
        actor
            .ask(ListSnapshotsMsg { deck_id, limit })
            .send()
            .await
            .map_err(map_send_err)
    }

    async fn deck_snapshot_get(
        &self,
        deck_id: String,
        snapshot_id: String,
    ) -> RpcResult<Option<DeckSnapshot>> {
        let actor = decks_actor()?;
        actor
            .ask(GetSnapshotMsg {
                deck_id,
                snapshot_id,
            })
            .send()
            .await
            .map_err(map_send_err)
    }

    async fn deck_subscribe(
        &self,
        pending: PendingSubscriptionSink,
        deck_id: String,
    ) -> SubscriptionResult {
        // Subscribe BEFORE accept so initial Sync and subsequent updates cannot race.
        let sub = async {
            let actor = decks_actor()?;
            actor
                .ask(SubscribeDeckMsg {
                    deck_id: deck_id.clone(),
                })
                .send()
                .await
                .map_err(map_send_err)
        };
        let (hub, sub_id, mut rx) = match sub.await {
            Ok(v) => v,
            Err(e) => {
                pending.reject(e).await;
                return Ok(());
            }
        };
        let sink = pending.accept().await?;
        let guard = UnsubGuard {
            hub,
            deck_id,
            sub_id,
        };
        tokio::spawn(async move {
            let _guard = guard;
            loop {
                let ev = tokio::select! {
                    _ = sink.closed() => return,
                    ev = rx.recv() => match ev { Some(ev) => ev, None => return },
                };
                let Ok(raw) = serde_json::value::to_raw_value(&ev) else {
                    continue;
                };
                let closed = matches!(ev, DeckRuntimeEvent::Closed { .. });
                if sink.send(SubscriptionMessage::from(raw)).await.is_err() || closed {
                    return;
                }
            }
        });
        Ok(())
    }
}

/// Assemble the `decks` plugin RPC module.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(DecksApiServer::into_rpc(server), rpc_descriptors())
        .expect("decks RPC descriptors")
}
