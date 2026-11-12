use agent::rpc_forward as forward;
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use jsonrpsee::types::ErrorObjectOwned;
use rpc_core::{Server, NOT_RUNNING, ask, dreaming_err};
use runtime::SessionId;
use decision::rpc_support::RpcDecisionExt;

#[rpc(server, client)]
pub trait DreamsApi {
    /// 57 §3.4 / §7.1: the decision-oracle switch of the families under `prefix` (`meeting`,
    /// `browser`) — `mode` as configured, `effective` (`off` with `reason` when the plugin is
    /// disabled or the checkpoint is missing), `enabled`, `installed`, and the tier budgets.
    #[method(name = "decision_mode")]
    async fn decision_mode(&self, prefix: String) -> RpcResult<decision::ModeInfo>;
    /// Persist `decision.families.<prefix>: { mode, train, share }` (`off` | `shadow` |
    /// `active`) and apply it live; effective from the next ask. A failed config write changes
    /// nothing. `browser` off ↔ on also changes whether `browser_run` is registered — that
    /// part takes a restart.
    #[method(name = "set_decision_mode")]
    async fn set_decision_mode(
        &self,
        prefix: String,
        mode: decision::Mode,
    ) -> RpcResult<decision::ModeInfo>;
    /// 126 §13: the double's status — enabled, paused, active version, last run, next window,
    /// venue, overall KPIs vs the frozen baseline.
    #[method(name = "double_status")]
    async fn double_status(&self) -> RpcResult<double::DoubleStatus>;
    /// Every discovered class with its rung, cap and last-30-days numbers (126 §13).
    #[method(name = "double_classes")]
    async fn double_classes(&self) -> RpcResult<Vec<double::DoubleClass>>;
    /// One class: the learning curve, the last 50 scrubbed examples, the promote hint (126 §13).
    #[method(name = "double_class")]
    async fn double_class(&self, key: String) -> RpcResult<double::DoubleClassDetail>;
    /// Start a training run now (126 §8): `tier` a | b | c, `venue` auto | local | node | off.
    /// Returns the run id; `DoubleTrainingState` events follow.
    #[method(name = "double_train")]
    async fn double_train(
        &self,
        tier: double::Tier,
        venue: double::Venue,
    ) -> RpcResult<double::RunId>;
    /// Pause / resume learning (126 §9): predictions and rungs continue, labels stop accruing.
    #[method(name = "double_pause")]
    async fn double_pause(&self, on: bool) -> RpcResult<double::DoubleStatus>;
    /// Forget a class's examples or specific examples (126 §14); returns the count removed.
    #[method(name = "double_forget")]
    async fn double_forget(&self, target: double::ForgetTarget) -> RpcResult<u32>;
    /// Make `version` the active one (126 §8.5); the next prediction uses it.
    #[method(name = "double_rollback")]
    async fn double_rollback(&self, version: String) -> RpcResult<double::DoubleStatus>;
    /// Set a class's cap and mutating opt-in (126 §9 caps); destructive stays ≤ 1.
    #[method(name = "double_set_cap")]
    async fn double_set_cap(
        &self,
        class: String,
        rung: u8,
        mutating_opt_in: bool,
    ) -> RpcResult<double::DoubleClass>;
    /// Write a scrubbed JSONL + embeddings archive under `<data_dir>/double/export/` (126 §14).
    #[method(name = "double_export")]
    async fn double_export(&self) -> RpcResult<double::DoubleExport>;
    /// Remove the store, models and runs; config untouched (126 §14).
    #[method(name = "double_delete_all")]
    async fn double_delete_all(&self) -> RpcResult<()>;
    /// Cancel an L2 grace so the owner answers through the ordinary `resolve_approval` /
    /// `answer_ask` (126 §9); `true` when a grace was running.
    #[method(name = "double_take_over")]
    async fn double_take_over(&self, question_id: String) -> RpcResult<bool>;
    /// The `double:` section as running (126 §12).
    #[method(name = "double_settings_get")]
    async fn double_settings_get(&self) -> RpcResult<double::DoubleConfig>;
    /// Persist the `double:` section and apply it live; `enabled` and `model.*` take a restart.
    #[method(name = "double_settings_set")]
    async fn double_settings_set(
        &self,
        settings: double::DoubleConfig,
    ) -> RpcResult<double::DoubleConfig>;
    /// 136 §2.7: dreaming's status — enabled, the last night, the next window, budget left tonight,
    /// backlog, proposals pending, paused_until. Answers from `state.json` even when the plugin is
    /// disabled (`enabled: false`, defaults otherwise).
    #[method(name = "dreaming.status")]
    async fn dreaming_status(&self) -> RpcResult<dreaming::DreamStatus>;
    /// The last `limit` nights, newest first (136 §2.7).
    #[method(name = "dreaming.nights")]
    async fn dreaming_nights(&self, limit: u32) -> RpcResult<Vec<dreaming::NightSummary>>;
    /// Dream now (136 §2.7): fire the routine's session with this run's budget and mode; returns the session.
    /// 185 A5: on a peered non-hub with the hub connected the press runs on the hub (forwarded).
    #[method(name = "dreaming.run_now", with_extensions)]
    async fn dreaming_run_now(&self, request: dreaming::RunRequest) -> RpcResult<SessionId>;
    /// Undo a night or one group of it (136 §2.7).
    #[method(name = "dreaming.undo")]
    async fn dreaming_undo(
        &self,
        night: String,
        group: Option<String>,
    ) -> RpcResult<dreaming::UndoReport>;
    /// Settings › Dreaming as running (136 §2.7); from the config section even when disabled.
    #[method(name = "dreaming.settings")]
    async fn dreaming_settings(&self) -> RpcResult<dreaming::DreamSettings>;
    /// Persist the `dreaming:` section through the settings path and apply it live (136 §2.7).
    #[method(name = "dreaming.set_settings")]
    async fn dreaming_set_settings(
        &self,
        settings: dreaming::DreamSettings,
    ) -> RpcResult<dreaming::DreamSettings>;
    /// Proposals waiting for the user (136 §2.7).
    #[method(name = "dreaming.proposals")]
    async fn dreaming_proposals(&self) -> RpcResult<Vec<dreaming::Proposal>>;
    /// The user pressed `choice` on a proposal (136 §2.7).
    #[method(name = "dreaming.proposal_act")]
    async fn dreaming_proposal_act(&self, id: String, choice: String) -> RpcResult<()>;
    /// The 136 §13 series over `days`.
    #[method(name = "dreaming.metrics")]
    async fn dreaming_metrics(&self, days: u32) -> RpcResult<dreaming::Metrics>;
    /// 57 §7: today's decision log folded per family — mode, asks, accepts, escalations by
    /// reason, tier-2 rows, p50/p95, shadow agreement.
    #[method(name = "decision_stats")]
    async fn decision_stats(&self) -> RpcResult<Vec<decision::FamilyStats>>;
    /// 57 §2.1 item 4: the decision checkpoints resident right now — `[]` at idle. `rss_mb`
    /// is the whole daemon's footprint, repeated on every entry.
    #[method(name = "decision_loaded")]
    async fn decision_loaded(&self) -> RpcResult<Vec<decision::LoadedInfo>>;
    /// 57 §11.4 / §7.1: whether the families under `prefix` (`meeting`, `browser`) capture
    /// training examples locally (`enabled`), consent to upload them (`share`), and how much is
    /// on disk (`<data_dir>/decisions/training/<prefix>/`).
    #[method(name = "decision_training")]
    async fn decision_training(&self, prefix: String) -> RpcResult<decision::TrainingInfo>;
    /// Persist `decision.families.<prefix>: { mode, train, share }` and apply it live; effective
    /// from the next batch. Turning capture off also clears `share`. A failed config write
    /// changes nothing.
    #[method(name = "set_decision_training")]
    async fn set_decision_training(
        &self,
        prefix: String,
        on: bool,
    ) -> RpcResult<decision::TrainingInfo>;
    /// 57 §7.1: consent to upload the prefix's captured rows, persisted the same way. Refused
    /// (`-32000`) when capture is off. No uploader reads it in this version.
    #[method(name = "set_decision_share")]
    async fn set_decision_share(
        &self,
        prefix: String,
        on: bool,
    ) -> RpcResult<decision::TrainingInfo>;
    /// 57 §7.1: remove `<data_dir>/decisions/training/<prefix>/`; the switches stay as set.
    /// Answers the new counts.
    #[method(name = "delete_decision_training")]
    async fn delete_decision_training(&self, prefix: String) -> RpcResult<decision::TrainingInfo>;
}

#[async_trait]
impl DreamsApiServer for Server {

    async fn decision_mode(&self, prefix: String) -> RpcResult<decision::ModeInfo> {
        RpcDecisionExt::decision_mode(self, prefix).await
    }

    async fn set_decision_mode(
        &self,
        prefix: String,
        mode: decision::Mode,
    ) -> RpcResult<decision::ModeInfo> {
        RpcDecisionExt::set_decision_mode(self, prefix, mode).await
    }

    // 126: the read / ladder methods ask the double actor (W1); `double_train`, `double_rollback`,
    // `double_export`, `double_delete_all` and `double_settings_set` ask the scheduler (W2). With
    // `double.enabled: false` neither actor exists and `lookup` answers `NOT_RUNNING`, never a panic.

    async fn double_status(&self) -> RpcResult<double::DoubleStatus> {
        match ask::<double::actor::Double, _>(double::DOUBLE_ACTOR, double::actor::Status).await {
            Ok(s) => Ok(s),
            // Disabled: the plugin registered nothing; report the section as read.
            Err(e) if e.code() == NOT_RUNNING => {
                let cfg = self.double_config();
                Ok(double::DoubleStatus {
                    enabled: cfg.enabled,
                    venue: cfg.training.venue,
                    ..double::DoubleStatus::default()
                })
            }
            Err(e) => Err(e),
        }
    }

    async fn double_classes(&self) -> RpcResult<Vec<double::DoubleClass>> {
        ask::<double::actor::Double, _>(double::DOUBLE_ACTOR, double::actor::Classes).await
    }

    async fn double_class(&self, key: String) -> RpcResult<double::DoubleClassDetail> {
        ask::<double::actor::Double, _>(double::DOUBLE_ACTOR, double::actor::ClassDetail(key)).await
    }

    async fn double_train(
        &self,
        tier: double::Tier,
        venue: double::Venue,
    ) -> RpcResult<double::RunId> {
        ask::<double::sched::Scheduler, _>(
            double::sched::ACTOR,
            double::sched::Train { tier, venue },
        )
        .await
    }

    async fn double_pause(&self, on: bool) -> RpcResult<double::DoubleStatus> {
        ask::<double::actor::Double, _>(double::DOUBLE_ACTOR, double::actor::Pause(on)).await
    }

    async fn double_forget(&self, target: double::ForgetTarget) -> RpcResult<u32> {
        ask::<double::actor::Double, _>(double::DOUBLE_ACTOR, double::actor::Forget(target)).await
    }

    async fn double_rollback(&self, version: String) -> RpcResult<double::DoubleStatus> {
        ask::<double::sched::Scheduler, _>(double::sched::ACTOR, double::sched::Rollback(version))
            .await
    }

    async fn double_set_cap(
        &self,
        class: String,
        rung: u8,
        mutating_opt_in: bool,
    ) -> RpcResult<double::DoubleClass> {
        ask::<double::actor::Double, _>(
            double::DOUBLE_ACTOR,
            double::actor::SetCap {
                class,
                rung,
                mutating_opt_in,
            },
        )
        .await
    }

    async fn double_export(&self) -> RpcResult<double::DoubleExport> {
        ask::<double::sched::Scheduler, _>(double::sched::ACTOR, double::sched::Export).await
    }

    async fn double_delete_all(&self) -> RpcResult<()> {
        ask::<double::sched::Scheduler, _>(double::sched::ACTOR, double::sched::DeleteAll).await
    }

    async fn double_take_over(&self, question_id: String) -> RpcResult<bool> {
        match ask::<double::actor::Double, _>(
            double::DOUBLE_ACTOR,
            double::actor::TakeOver(question_id),
        )
        .await
        {
            Ok(b) => Ok(b),
            // No actor: no grace can be running; nothing to cancel.
            Err(e) if e.code() == NOT_RUNNING => Ok(false),
            Err(e) => Err(e),
        }
    }

    async fn double_settings_get(&self) -> RpcResult<double::DoubleConfig> {
        Ok(self.double_config())
    }

    async fn double_settings_set(
        &self,
        settings: double::DoubleConfig,
    ) -> RpcResult<double::DoubleConfig> {
        ask::<double::sched::Scheduler, _>(
            double::sched::ACTOR,
            double::sched::SetSettings(settings),
        )
        .await
    }

    // 136 §2.7: every method asks the `dreaming` actor through `dreaming::rpc`; with
    // `dreaming.enabled: false` the actor does not exist — `status` and `settings` then answer from
    // the section and `state.json`, the rest report `NOT_RUNNING`.

    async fn dreaming_status(&self) -> RpcResult<dreaming::DreamStatus> {
        match dreaming::rpc::status().await {
            Ok(s) => Ok(s),
            Err(_) => Ok(dreaming::night::status(
                &self.rt.data_dir,
                &self.dreaming_config(),
            )),
        }
    }

    async fn dreaming_nights(&self, limit: u32) -> RpcResult<Vec<dreaming::NightSummary>> {
        dreaming::rpc::nights(limit).await.map_err(dreaming_err)
    }

    async fn dreaming_run_now(
        &self,
        ext: &Extensions,
        request: dreaming::RunRequest,
    ) -> RpcResult<SessionId> {
        // 185 A5: the hub dreams; forwarded once (`to_peer` never makes a second
        // hop), and run here when the hub is not connected (this side is cut off).
        if let Some(hub) = dreaming::rpc::run_now_host().await
            && let Some(s) =
                forward::to_peer(ext, &replica::PeerId(hub), "dreaming.run_now", [&request])
                    .await
                    .map_err(hub_said)?
        {
            return Ok(s);
        }
        dreaming::rpc::run_now(request).await.map_err(dreaming_err)
    }

    async fn dreaming_undo(
        &self,
        night: String,
        group: Option<String>,
    ) -> RpcResult<dreaming::UndoReport> {
        dreaming::rpc::undo(night, group)
            .await
            .map_err(dreaming_err)
    }

    async fn dreaming_settings(&self) -> RpcResult<dreaming::DreamSettings> {
        match dreaming::rpc::settings().await {
            Ok(s) => Ok(s),
            Err(_) => Ok(dreaming::night::settings(
                &self.rt.data_dir,
                &self.dreaming_config(),
                &dreaming::config::AgentDefaults::from_runtime(&self.rt),
            )),
        }
    }

    async fn dreaming_set_settings(
        &self,
        settings: dreaming::DreamSettings,
    ) -> RpcResult<dreaming::DreamSettings> {
        dreaming::rpc::set_settings(settings)
            .await
            .map_err(dreaming_err)
    }

    async fn dreaming_proposals(&self) -> RpcResult<Vec<dreaming::Proposal>> {
        dreaming::rpc::proposals().await.map_err(dreaming_err)
    }

    async fn dreaming_proposal_act(&self, id: String, choice: String) -> RpcResult<()> {
        dreaming::rpc::proposal_act(id, choice)
            .await
            .map_err(dreaming_err)
    }

    async fn dreaming_metrics(&self, days: u32) -> RpcResult<dreaming::Metrics> {
        dreaming::rpc::metrics(days).await.map_err(dreaming_err)
    }

    async fn decision_stats(&self) -> RpcResult<Vec<decision::FamilyStats>> {
        ask::<decision::Engine, _>(decision::DECISION_ACTOR, decision::AllStats).await
    }

    async fn decision_loaded(&self) -> RpcResult<Vec<decision::LoadedInfo>> {
        ask::<decision::Engine, _>(decision::DECISION_ACTOR, decision::LoadedNow).await
    }

    async fn decision_training(&self, prefix: String) -> RpcResult<decision::TrainingInfo> {
        ask::<decision::Engine, _>(decision::DECISION_ACTOR, decision::GetTrain { prefix }).await
    }

    async fn set_decision_training(
        &self,
        prefix: String,
        on: bool,
    ) -> RpcResult<decision::TrainingInfo> {
        ask::<decision::Engine, _>(decision::DECISION_ACTOR, decision::SetTrain { prefix, on })
            .await
    }

    async fn set_decision_share(
        &self,
        prefix: String,
        on: bool,
    ) -> RpcResult<decision::TrainingInfo> {
        ask::<decision::Engine, _>(decision::DECISION_ACTOR, decision::SetShare { prefix, on })
            .await
    }

    async fn delete_decision_training(&self, prefix: String) -> RpcResult<decision::TrainingInfo> {
        ask::<decision::Engine, _>(
            decision::DECISION_ACTOR,
            decision::DeleteTraining { prefix },
        )
        .await
    }
}



pub(super) trait DreamConfigExt {
    fn double_config(&self) -> double::DoubleConfig;
    fn dreaming_config(&self) -> dreaming::DreamConfig;
}

impl DreamConfigExt for Server {
    /// The `double:` section as read at start (126 §12); defaults when absent.
    fn double_config(&self) -> double::DoubleConfig {
        self.rt.section("double").unwrap_or_default()
    }

    /// The `dreaming:` section as read at start (136 §9.2); defaults when absent.
    fn dreaming_config(&self) -> dreaming::DreamConfig {
        self.rt.section("dreaming").unwrap_or_default()
    }

}
/// 185 R1 A5: the hub's refusal of a forwarded Dream now, said as the hub's —
/// "Dream now runs on the hub, which …" — with the code and data kept (a
/// `-32010` still carries `OwnerUnreachable`). Nothing runs here instead: the
/// hub may have started the night.
fn hub_said(e: ErrorObjectOwned) -> ErrorObjectOwned {
    let msg = if e.code() == forward::OWNER_UNREACHABLE {
        format!(
            "Dream now runs on the hub, which did not answer ({}); try again when it is back",
            e.message()
        )
    } else {
        format!("Dream now runs on the hub, which said: {}", e.message())
    };
    ErrorObjectOwned::owned(e.code(), msg, e.data().map(|d| d.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rpc_core::ASK_FAILED;

    /// 185 R1 A5: a hub that refuses or does not answer is named as the hub;
    /// the code and the `OwnerUnreachable` payload stay for the UI.
    #[test]
    fn the_hubs_refusal_of_dream_now_is_said_as_the_hubs() {
        let off = hub_said(ErrorObjectOwned::owned(
            ASK_FAILED,
            "dreaming is off in config (dreaming.enabled: false)",
            None::<()>,
        ));
        assert_eq!(off.code(), ASK_FAILED);
        assert_eq!(
            off.message(),
            "Dream now runs on the hub, which said: dreaming is off in config (dreaming.enabled: false)"
        );
        let gone = hub_said(ErrorObjectOwned::owned(
            forward::OWNER_UNREACHABLE,
            "no answer within 30s",
            Some(forward::FrontDoorError::OwnerUnreachable {
                owner: "hub".into(),
                last_seen: None,
            }),
        ));
        assert_eq!(gone.code(), forward::OWNER_UNREACHABLE);
        assert!(
            gone.message()
                .starts_with("Dream now runs on the hub, which did not answer"),
            "{}",
            gone.message()
        );
        assert!(
            gone.data()
                .is_some_and(|d| d.get().contains("OwnerUnreachable"))
        );
    }
}
