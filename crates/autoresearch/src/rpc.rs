use crate as autoresearch;
use crate::landing;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::{Server, dreaming_err};

#[rpc_core_macros::rpc_descriptors(owner = "autoresearch", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait ResearchApi {
    /// 140 §12.5: every study, newest first.
    #[method(name = "autoresearch.list")]
    async fn autoresearch_list(&self) -> RpcResult<Vec<autoresearch::StudyInfo>>;
    /// 140 §12.5: one study with its spec, incumbent, σ, budget and chart series.
    #[method(name = "autoresearch.get")]
    async fn autoresearch_get(&self, slug: String) -> RpcResult<autoresearch::StudyDetail>;
    /// 140 §12.5: run-table rows newest first; `before` pages.
    #[method(name = "autoresearch.experiments")]
    async fn autoresearch_experiments(
        &self,
        slug: String,
        before: Option<u32>,
        limit: u32,
    ) -> RpcResult<Vec<autoresearch::ExperimentRow>>;
    /// 140 §12.5: one experiment — samples, verdict line, deviations, review, diff, log tails.
    #[method(name = "autoresearch.experiment")]
    async fn autoresearch_experiment(
        &self,
        slug: String,
        n: u32,
    ) -> RpcResult<autoresearch::ExperimentDetail>;
    /// 140 §12.5 / §8.6: pause, resume, stop_after, stop_now, run_once, segment, finalize, restore, discard.
    #[method(name = "autoresearch.control")]
    async fn autoresearch_control(
        &self,
        slug: String,
        op: autoresearch::ControlOp,
    ) -> RpcResult<autoresearch::StudyInfo>;
    /// 140 §7.3: a steer reaches the next brief without pausing; returns the row id.
    #[method(name = "autoresearch.steer")]
    async fn autoresearch_steer(&self, slug: String, text: String) -> RpcResult<i64>;
    /// 140 §7.3: add, reprioritise, close or reopen a backlog idea.
    #[method(name = "autoresearch.idea")]
    async fn autoresearch_idea(
        &self,
        slug: String,
        op: autoresearch::IdeaChange,
    ) -> RpcResult<autoresearch::Idea>;
    /// 140 §12.4: the draft card's Start press — creates the study from the draft.
    #[method(name = "autoresearch.create")]
    async fn autoresearch_create(
        &self,
        draft_id: String,
        start: autoresearch::StartWhen,
    ) -> RpcResult<autoresearch::StudyInfo>;
    /// Settings › Autoresearch as running (140 §12.2); from the config section even when disabled.
    #[method(name = "autoresearch.settings.get")]
    async fn autoresearch_settings_get(&self) -> RpcResult<autoresearch::AutoresearchConfig>;
    /// Persist the `autoresearch:` section and apply it live (140 §12.5).
    #[method(name = "autoresearch.settings.set")]
    async fn autoresearch_settings_set(
        &self,
        config: autoresearch::AutoresearchConfig,
    ) -> RpcResult<autoresearch::AutoresearchConfig>;
    /// 151 §4.4: the live draft panel — rows with engine-computed states, `ready`, what blocks Start.
    #[method(name = "autoresearch.draft")]
    async fn autoresearch_draft(&self, draft_id: String) -> RpcResult<autoresearch::DraftView>;
    /// 151 D6: drafts with no study yet (the rail's "Setting up" rows), newest first.
    #[method(name = "autoresearch.drafts")]
    async fn autoresearch_drafts(&self) -> RpcResult<Vec<autoresearch::DraftSummary>>;
    /// 151 D3: a leaf set in the panel (`metric.name`, `scope`, …; text, parsed by the engine) — user-set and confirmed.
    #[method(name = "autoresearch.draft_edit")]
    async fn autoresearch_draft_edit(
        &self,
        draft_id: String,
        field: String,
        value: String,
    ) -> RpcResult<autoresearch::DraftView>;
    /// 151 D3: a row OK'd in the panel (`proposed` → `confirmed`).
    #[method(name = "autoresearch.draft_confirm")]
    async fn autoresearch_draft_confirm(
        &self,
        draft_id: String,
        field: String,
    ) -> RpcResult<autoresearch::DraftView>;
    /// 151 D3: discard a draft — the drafts dir is removed, later Starts are refused.
    #[method(name = "autoresearch.discard_draft")]
    async fn autoresearch_discard_draft(&self, draft_id: String) -> RpcResult<()>;
    /// 159 §5.11: archive rows — front members (W1) or population cells (W4).
    #[method(name = "autoresearch.archive")]
    async fn autoresearch_archive(&self, slug: String) -> RpcResult<Vec<autoresearch::ArchiveRow>>;
    /// 159 §5.11: case × experiment rows for the heat strip; `n` narrows to one experiment (W2).
    #[method(name = "autoresearch.cases")]
    async fn autoresearch_cases(
        &self,
        slug: String,
        n: Option<u32>,
    ) -> RpcResult<Vec<autoresearch::CaseRow>>;
    /// 159 §5.11: blind-pair verdicts; `n` narrows to one experiment (W3).
    #[method(name = "autoresearch.judgments")]
    async fn autoresearch_judgments(
        &self,
        slug: String,
        n: Option<u32>,
    ) -> RpcResult<Vec<autoresearch::JudgmentRow>>;
    /// 159 §5.11: the user's answer (`a` | `b` | `same`) to blind pair `id` (W3).
    #[method(name = "autoresearch.human_pair")]
    async fn autoresearch_human_pair(
        &self,
        slug: String,
        id: i64,
        winner: String,
    ) -> RpcResult<autoresearch::StudyInfo>;
    /// 159 §5.11: the launch press — approve deploys both arms, refuse discards experiment `n` (W6).
    #[method(name = "autoresearch.launch")]
    async fn autoresearch_launch(
        &self,
        slug: String,
        n: u32,
        token: String,
        approve: bool,
    ) -> RpcResult<autoresearch::StudyInfo>;
    /// 159 §5.11: list, show or adopt a mechanism version (W7).
    #[method(name = "autoresearch.mechanism")]
    async fn autoresearch_mechanism(
        &self,
        op: autoresearch::MechanismOp,
    ) -> RpcResult<autoresearch::MechanismInfo>;
}

#[async_trait]
impl ResearchApiServer for Server {

    // 140 §12.5: every method asks the `autoresearch` actor through `autoresearch::rpc_impl`
    // (`dreaming_err` maps "is not running" to `NOT_RUNNING`); `list` is empty and
    // `settings.get` / `settings.set` answer from the section when the plugin is disabled, so
    // the Library tab and Settings › Autoresearch work (and can re-enable it) without the actor.

    async fn autoresearch_list(&self) -> RpcResult<Vec<autoresearch::StudyInfo>> {
        match autoresearch::rpc_impl::list().await {
            Err(e) if e.ends_with("is not running") => Ok(Vec::new()),
            r => r.map_err(dreaming_err),
        }
    }

    async fn autoresearch_get(&self, slug: String) -> RpcResult<autoresearch::StudyDetail> {
        autoresearch::rpc_impl::get(slug)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_experiments(
        &self,
        slug: String,
        before: Option<u32>,
        limit: u32,
    ) -> RpcResult<Vec<autoresearch::ExperimentRow>> {
        autoresearch::rpc_impl::experiments(slug, before, limit)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_experiment(
        &self,
        slug: String,
        n: u32,
    ) -> RpcResult<autoresearch::ExperimentDetail> {
        autoresearch::rpc_impl::experiment(slug, n)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_control(
        &self,
        slug: String,
        op: autoresearch::ControlOp,
    ) -> RpcResult<autoresearch::StudyInfo> {
        // 159 §13.3: a learned-sidecar study's Merge lands through dreaming first (backup,
        // atomic write, `edits.jsonl`); the actor's Merge then does its bookkeeping.
        landing::before_merge(&self.rt.data_dir, &slug, &op)
            .await
            .map_err(dreaming_err)?;
        autoresearch::rpc_impl::control(slug, op)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_steer(&self, slug: String, text: String) -> RpcResult<i64> {
        autoresearch::rpc_impl::steer(slug, text)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_idea(
        &self,
        slug: String,
        op: autoresearch::IdeaChange,
    ) -> RpcResult<autoresearch::Idea> {
        autoresearch::rpc_impl::idea(slug, op)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_create(
        &self,
        draft_id: String,
        start: autoresearch::StartWhen,
    ) -> RpcResult<autoresearch::StudyInfo> {
        autoresearch::rpc_impl::create(draft_id, start)
            .await
            .map_err(dreaming_err)
    }

    // 151 §4.4: the living draft (bodies in `autoresearch::rpc_impl`, built by ENG).

    async fn autoresearch_draft(&self, draft_id: String) -> RpcResult<autoresearch::DraftView> {
        autoresearch::rpc_impl::draft(draft_id)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_drafts(&self) -> RpcResult<Vec<autoresearch::DraftSummary>> {
        match autoresearch::rpc_impl::drafts().await {
            Err(e) if e.ends_with("is not running") => Ok(Vec::new()),
            r => r.map_err(dreaming_err),
        }
    }

    async fn autoresearch_draft_edit(
        &self,
        draft_id: String,
        field: String,
        value: String,
    ) -> RpcResult<autoresearch::DraftView> {
        autoresearch::rpc_impl::draft_edit(draft_id, field, value)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_draft_confirm(
        &self,
        draft_id: String,
        field: String,
    ) -> RpcResult<autoresearch::DraftView> {
        autoresearch::rpc_impl::draft_confirm(draft_id, field)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_discard_draft(&self, draft_id: String) -> RpcResult<()> {
        autoresearch::rpc_impl::discard_draft(draft_id)
            .await
            .map_err(dreaming_err)
    }

    // 159 §5.11: per-kind methods; stubs in `autoresearch::rpc_impl` until their wave.

    async fn autoresearch_archive(&self, slug: String) -> RpcResult<Vec<autoresearch::ArchiveRow>> {
        autoresearch::rpc_impl::archive(slug)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_cases(
        &self,
        slug: String,
        n: Option<u32>,
    ) -> RpcResult<Vec<autoresearch::CaseRow>> {
        autoresearch::rpc_impl::cases(slug, n)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_judgments(
        &self,
        slug: String,
        n: Option<u32>,
    ) -> RpcResult<Vec<autoresearch::JudgmentRow>> {
        autoresearch::rpc_impl::judgments(slug, n)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_human_pair(
        &self,
        slug: String,
        id: i64,
        winner: String,
    ) -> RpcResult<autoresearch::StudyInfo> {
        autoresearch::rpc_impl::human_pair(slug, id, winner)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_launch(
        &self,
        slug: String,
        n: u32,
        token: String,
        approve: bool,
    ) -> RpcResult<autoresearch::StudyInfo> {
        autoresearch::rpc_impl::launch(slug, n, token, approve)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_mechanism(
        &self,
        op: autoresearch::MechanismOp,
    ) -> RpcResult<autoresearch::MechanismInfo> {
        autoresearch::rpc_impl::mechanism(op)
            .await
            .map_err(dreaming_err)
    }

    async fn autoresearch_settings_get(&self) -> RpcResult<autoresearch::AutoresearchConfig> {
        match autoresearch::rpc_impl::settings().await {
            Ok(c) => Ok(c),
            Err(_) => Ok(self.autoresearch_config()),
        }
    }

    async fn autoresearch_settings_set(
        &self,
        config: autoresearch::AutoresearchConfig,
    ) -> RpcResult<autoresearch::AutoresearchConfig> {
        match autoresearch::rpc_impl::set_settings(config.clone()).await {
            Err(e) if e.ends_with("is not running") => {
                autoresearch::config::persist(&self.rt.data_dir.join("config.yaml"), &config)
                    .map_err(|e| dreaming_err(format!("config.yaml: {e}")))?;
                Ok(config)
            }
            r => r.map_err(dreaming_err),
        }
    }
}

pub(super) trait ResearchConfigExt {
    fn autoresearch_config(&self) -> autoresearch::AutoresearchConfig;
}

impl ResearchConfigExt for Server {
    /// The `autoresearch:` section as read at start (140 §12.2); defaults when absent.
    fn autoresearch_config(&self) -> autoresearch::AutoresearchConfig {
        self.rt.section("autoresearch").unwrap_or_default()
    }

}

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(ResearchApiServer::into_rpc(server), rpc_descriptors())
        .expect("autoresearch RPC descriptors")
}
