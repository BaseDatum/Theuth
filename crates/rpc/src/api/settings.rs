use super::*;

#[rpc(server, client)]
pub trait SettingsApi {
    /// The live default compaction style (`agent.compact_style`).
    #[method(name = "compact_style")]
    async fn compact_style(&self) -> RpcResult<CompactStyle>;
    /// Persist `agent.compact_style` and update live sessions.
    #[method(name = "set_compact_style")]
    async fn set_compact_style(&self, style: CompactStyle) -> RpcResult<()>;
    /// Persist `agent.subagents.max_runtime_secs` and use it for newly spawned turns.
    #[method(name = "set_subagent_timeout")]
    async fn set_subagent_timeout(&self, seconds: u64) -> RpcResult<()>;
    /// Persist `agent.max_tool_rounds`; new sessions use it.
    #[method(name = "set_max_tool_rounds")]
    async fn set_max_tool_rounds(&self, rounds: u32) -> RpcResult<()>;
    /// Persist `agent.stream_idle_secs`; new sessions use it.
    #[method(name = "set_stream_idle")]
    async fn set_stream_idle(&self, seconds: u64) -> RpcResult<()>;
    /// Persist `agent.max_salvages`; new sessions use it.
    #[method(name = "set_max_salvages")]
    async fn set_max_salvages(&self, count: u32) -> RpcResult<()>;
    /// Persist `tools.pty_idle_secs` (61 §1.6); `start` calls without `idle_secs` use it from then on. `0` = never.
    #[method(name = "set_pty_idle")]
    async fn set_pty_idle(&self, seconds: u64) -> RpcResult<()>;
    /// The live title model (`agent.title_model`), if set.
    #[method(name = "title_model")]
    async fn title_model(&self) -> RpcResult<Option<ModelInfo>>;
    /// Persist `agent.title_model`; `None` clears it (titles use the session's model).
    #[method(name = "set_title_model")]
    async fn set_title_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>>;
    /// 194: the live title-call effort (`agent.title_effort`); `None` = no reasoning setting.
    #[method(name = "title_effort")]
    async fn title_effort(&self) -> RpcResult<Option<String>>;
    /// Persist `agent.title_effort`; refused when the title model does not take it; `None`
    /// clears it. Returns the stored value. A title-model change drops an effort it does not take.
    #[method(name = "set_title_effort")]
    async fn set_title_effort(&self, effort: Option<String>) -> RpcResult<Option<String>>;
    /// The live polish model (`voice.polish_model`, 153 §9.2), if set.
    #[method(name = "polish_model")]
    async fn polish_model(&self) -> RpcResult<Option<ModelInfo>>;
    /// Persist `voice.polish_model`; `None` clears it (polish uses the title model, then the session's).
    #[method(name = "set_polish_model")]
    async fn set_polish_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>>;
    /// 194: the live polish effort (`voice.polish_effort`), sent when the polish model answers.
    #[method(name = "polish_effort")]
    async fn polish_effort(&self) -> RpcResult<Option<String>>;
    /// Persist `voice.polish_effort`; refused when the model polish resolves to does not take
    /// it; `None` clears it. Returns the stored value.
    #[method(name = "set_polish_effort")]
    async fn set_polish_effort(&self, effort: Option<String>) -> RpcResult<Option<String>>;
    /// The live vision fallback model (`agent.vision_model`), if set (52 §3.1).
    #[method(name = "vision_model")]
    async fn vision_model(&self) -> RpcResult<Option<ModelInfo>>;
    /// Persist `agent.vision_model`; `None` clears it (no fallback). Rejects
    /// anything but an exact, image-capable catalog entry on a configured
    /// provider; a failed config write is an error and changes nothing.
    #[method(name = "set_vision_model")]
    async fn set_vision_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>>;
    /// 194: the live vision-fallback effort (`agent.vision_effort`); `None` = the model's default.
    #[method(name = "vision_effort")]
    async fn vision_effort(&self) -> RpcResult<Option<String>>;
    /// Persist `agent.vision_effort`; refused when the fallback model does not take it; `None`
    /// clears it. Returns the stored value. Vision Off clears it too.
    #[method(name = "set_vision_effort")]
    async fn set_vision_effort(&self, effort: Option<String>) -> RpcResult<Option<String>>;
    /// The escalation model (`agent.escalate_model`, 57 §0.9: tier 2 of the
    /// local decision oracle) and what a tier-2 call resolves to today.
    #[method(name = "escalate_model")]
    async fn escalate_model(&self) -> RpcResult<EscalateModel>;
    /// Persist `agent.escalate_model`; `None` clears it (inherits the title
    /// model, then the session's model). Rejects an id that is not a catalog
    /// entry; a failed config write is an error and changes nothing.
    #[method(name = "set_escalate_model")]
    async fn set_escalate_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>>;
    /// 194: the live escalation effort (`agent.escalate_effort`), sent when the escalation
    /// model answers a tier-2 call.
    #[method(name = "escalate_effort")]
    async fn escalate_effort(&self) -> RpcResult<Option<String>>;
    /// Persist `agent.escalate_effort`; refused when the effective escalation model does not
    /// take it; `None` clears it. Returns the stored value. Escalation Off clears it too.
    #[method(name = "set_escalate_effort")]
    async fn set_escalate_effort(&self, effort: Option<String>) -> RpcResult<Option<String>>;
    /// The live subagent default model (`agent.subagents.model`), if set.
    #[method(name = "subagent_model")]
    async fn subagent_model(&self) -> RpcResult<Option<ModelInfo>>;
    /// Persist `agent.subagents.model`; `None` clears it (children use `agent.model`). Native
    /// children spawned without `model` start on it and the system prompt names it (148 R2).
    #[method(name = "set_subagent_model")]
    async fn set_subagent_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>>;
    /// Every catalog model whose provider is configured. 183 W-12: `engine`
    /// (mesh id) lists that engine's catalog; `None` = this engine.
    #[method(name = "list_models", with_extensions)]
    async fn list_models(&self, engine: Option<String>) -> RpcResult<Vec<ModelInfo>>;
    /// AWS SDK-discovered profiles on this harness, plus its inference selection.
    #[method(name = "list_aws_profiles")]
    async fn list_aws_profiles(&self) -> RpcResult<AwsProfiles>;
    /// Select the harness-wide Bedrock inference profile; `None` restores the AWS default chain.
    #[method(name = "set_aws_profile")]
    async fn set_aws_profile(&self, profile: Option<String>) -> RpcResult<AwsProfiles>;
    /// With `session`: switch that session to `model` — that session only, whatever `manual`
    /// says (148 R1). With no session and `manual` (default `true`): set the default for new
    /// sessions (`agent.model`); `DefaultModelChanged` follows on the stream. With no session
    /// and `manual: false`: resolve `model` and report it, nothing else.
    #[method(name = "set_model", with_extensions)]
    async fn set_model(
        &self,
        session: Option<SessionId>,
        model: String,
        manual: Option<bool>,
    ) -> RpcResult<ModelInfo>;
    /// Star (`favorite: true`) or unstar `model` for every device on this engine (127 §2).
    /// `model` resolves like `set_model` (unknown ids fail the same way). Returns the new
    /// list in starring order; `FavoriteModelsChanged` follows on the stream when it changed.
    #[method(name = "set_model_favorite")]
    async fn set_model_favorite(&self, model: String, favorite: bool) -> RpcResult<Vec<String>>;
    /// Set (or clear with `None`) the reasoning effort. Values come from the model's catalog
    /// entry (`ModelInfo.efforts`, or an integer token budget within `budget_min..budget_max`).
    /// With `session`: that session from its next turn, that session only, whatever `manual`
    /// says (148 R1); `EffortChanged` follows on the stream. With no session and `manual`
    /// (default `true`): the default for new sessions (`agent.effort`); `DefaultEffortChanged`
    /// follows.
    #[method(name = "set_effort", with_extensions)]
    async fn set_effort(
        &self,
        session: Option<SessionId>,
        effort: Option<String>,
        manual: Option<bool>,
    ) -> RpcResult<()>;
    /// 125 §6.3: turn the advisor of `session` on with `model` (`provider/model`; creates the
    /// hidden advisor child or re-picks its model) or off with `None`. The pick is remembered as
    /// `advisor.model` / `advisor.effort`; `AdvisorChanged` follows on the stream.
    #[method(name = "set_advisor", with_extensions)]
    async fn set_advisor(
        &self,
        session: SessionId,
        model: Option<String>,
        effort: Option<String>,
    ) -> RpcResult<AdvisorStatus>;
    /// 125 §6.3: the advisor status of `session` (`state: off` when it has none).
    #[method(name = "advisor_status", with_extensions)]
    async fn advisor_status(&self, session: SessionId) -> RpcResult<AdvisorStatus>;
    /// Persist `advisor.effort` (`None` clears); the remembered pick's effort (125 §6.5).
    #[method(name = "set_advisor_default_effort")]
    async fn set_advisor_default_effort(
        &self,
        effort: Option<String>,
    ) -> RpcResult<AdvisorDefaults>;
    /// Persist `advisor.max_notes_per_update` (1–32).
    #[method(name = "set_advisor_notes_per_update")]
    async fn set_advisor_notes_per_update(&self, n: u8) -> RpcResult<AdvisorDefaults>;
    /// Persist `advisor.immune_turns` (0–10; 0 disables the window).
    #[method(name = "set_advisor_immune_turns")]
    async fn set_advisor_immune_turns(&self, n: u8) -> RpcResult<AdvisorDefaults>;
    /// Persist `advisor.sync_backlog` (`off` | `1` | `3` | `5`).
    #[method(name = "set_advisor_sync_backlog")]
    async fn set_advisor_sync_backlog(&self, v: SyncBacklog) -> RpcResult<AdvisorDefaults>;
    /// Persist `advisor.max_consults` (0–32; 0 removes the `consult` tool) (125 R1).
    #[method(name = "set_advisor_max_consults")]
    async fn set_advisor_max_consults(&self, n: u8) -> RpcResult<AdvisorDefaults>;
    /// An ACP child's harness config options (75 §8): model, effort/thinking,
    /// mode … as the harness lists them, with current values. Errors when the
    /// session is not an external harness or its process is gone.
    #[method(name = "harness_options", with_extensions)]
    async fn harness_options(&self, session: SessionId) -> RpcResult<Vec<acp::ConfigOption>>;
    /// Set one harness config option on an ACP child (75 §8): `config_id` is
    /// the option's id or the alias `model` / `effort`; `value` one of its
    /// `options[].value`. Mid-turn the change is queued for the turn's end
    /// (`pending: true`). A missing option or an unlisted value is refused
    /// with the list. The model change follows as `HarnessModelChanged`.
    #[method(name = "set_harness_option", with_extensions)]
    async fn set_harness_option(
        &self,
        session: SessionId,
        config_id: String,
        value: String,
    ) -> RpcResult<acp::SetOptionResult>;
    /// Resume a stopped ACP child in its prior harness context (75 §10, 146
    /// D8): a new harness process, `session/load` on the stored ACP session
    /// id; a still-live process for the session is killed first. A child that
    /// was mid-prompt when it died is prompted to continue (`prompted`), an
    /// idle or cancelled one is only reconnected (`idle`). Errors when the
    /// session is native, belongs to a remote node, or has no stored harness
    /// session id.
    #[method(name = "resume_child", with_extensions)]
    async fn resume_child(&self, session: SessionId) -> RpcResult<ChildResumed>;
    /// Configured external harnesses (`acp/agents.yaml`); `probe` runs `initialize` on each.
    #[method(name = "list_acp")]
    async fn list_acp(&self, probe: bool) -> RpcResult<Vec<acp::EntryInfo>>;
    /// Install a registry agent as a harness entry (`binary` distributions are downloaded).
    #[method(name = "add_acp")]
    async fn add_acp(&self, id: String) -> RpcResult<acp::EntryInfo>;
    /// Remove a harness entry and its downloaded binary, if any.
    #[method(name = "remove_acp")]
    async fn remove_acp(&self, name: String) -> RpcResult<()>;
    /// The ACP agent registry (cached for a day; offline returns the cache).
    #[method(name = "list_acp_registry")]
    async fn list_acp_registry(&self) -> RpcResult<Vec<acp::registry::RegistryAgent>>;
    /// Providers a user can log in to (design doc 05), with their status.
    #[method(name = "list_logins")]
    async fn list_logins(&self) -> RpcResult<Vec<LoginInfo>>;
    /// Start a provider login; returns the URL to open. Ends with a `LoginFinished` event.
    #[method(name = "login")]
    async fn login(&self, provider: String) -> RpcResult<String>;
    /// Forget a provider login.
    #[method(name = "logout")]
    async fn logout(&self, provider: String) -> RpcResult<()>;
    /// The Connections providers: built-ins (`opencode`, `openrouter`) first, then custom providers (`/provider`).
    #[method(name = "list_providers")]
    async fn list_providers(&self) -> RpcResult<Vec<ProviderInfo>>;
    /// Persist a custom provider (url + optional key + optional local runtime preset);
    /// fetches `/models` with the context windows the server reports (177).
    /// A missing key is allowed for a local provider: its model calls carry the
    /// placeholder `Bearer local`, its listing no `Authorization` header.
    #[method(name = "add_provider")]
    async fn add_provider(
        &self,
        name: String,
        base_url: String,
        key: Option<String>,
        runtime: Option<agent::LocalRuntime>,
    ) -> RpcResult<ProviderInfo>;
    /// Forget a custom provider.
    #[method(name = "remove_provider")]
    async fn remove_provider(&self, name: String) -> RpcResult<()>;
    /// List a provider's models again (custom: `GET {base}models`; opencode/openrouter: their list) and reload the catalog (162).
    #[method(name = "refresh_provider_models")]
    async fn refresh_provider_models(&self, name: String) -> RpcResult<RefreshedModels>;
    /// 177 E8: what answers on the default local runtime ports of 127.0.0.1
    /// (11434, 1234, 8000, 8080); 1.5 s at most. Never adds anything.
    #[method(name = "local_servers_probe")]
    async fn local_servers_probe(&self) -> RpcResult<Vec<agent::LocalServer>>;
    /// 177: a provider's models with their stored limits (0 = unknown).
    #[method(name = "provider_models")]
    async fn provider_models(&self, provider: String) -> RpcResult<Vec<agent::ProviderModel>>;
    /// 177 E6: set a model's limits (source `user`; `None` clears back to unknown)
    /// and reload the catalog; running chats pick them up at their next turn.
    /// (`context` is renamed in Rust only: jsonrpsee's server glue owns that identifier.)
    #[method(name = "set_model_limits")]
    async fn set_model_limits(
        &self,
        provider: String,
        model: String,
        #[argument(rename = "context")] window: Option<u32>,
        output: Option<u32>,
    ) -> RpcResult<agent::ProviderModel>;
    /// 177 E9: one real summary (compactor) call for `model` on a fixed sample;
    /// writes no memory. Failures come back in the reply, with a hint.
    #[method(name = "test_summary_call")]
    async fn test_summary_call(&self, model: String) -> RpcResult<agent::SummaryTest>;
    /// The self-updater's latest state: `null`, or the version that is on disk
    /// (`applied`) / available but not writable (39 §4.3).
    #[method(name = "update_status")]
    async fn update_status(&self) -> RpcResult<Option<update::UpdateReady>>;
    /// 137 §11.1 `agent.endless` as in force (Settings › Memory).
    #[method(name = "endless_settings_get")]
    async fn endless_settings_get(&self) -> RpcResult<EndlessConfig>;
    /// Persist the whole `agent.endless` block to config.yaml and apply it
    /// live (defaults for new chats/desks/meetings, view budget, slack,
    /// settle). A changed compactor model applies to every attached session
    /// at once (137F). A failed write changes nothing.
    #[method(name = "endless_settings_set")]
    async fn endless_settings_set(&self, config: EndlessConfig) -> RpcResult<EndlessConfig>;
    /// 188: one [`ConfigFlagKey`] (`decision.enabled`, `a2a.enabled`, `computer.enabled`,
    /// `double.enabled`, `web.allow_private_network`, `browser.allow_private_network`) as
    /// config.yaml holds it and as this engine started with it. Node-level, never
    /// forwarded; any other key is `INVALID_PARAMS` naming them.
    #[method(name = "config_flag_get")]
    fn config_flag_get(&self, key: ConfigFlagKey) -> RpcResult<ConfigFlag>;
    /// 188: write only that one key (`<section>.enabled` or
    /// `<section>.allow_private_network`) into config.yaml (every other line,
    /// comments included, untouched). Nothing changes live: the plugin keeps its boot
    /// state until theuth restarts, so the reply's `restart_required` says whether one is due.
    #[method(name = "config_flag_set")]
    fn config_flag_set(&self, key: ConfigFlagKey, on: bool) -> RpcResult<ConfigFlag>;
    /// One probe query through one registered provider (built-in or external):
    /// ok, latency, rows, error text. Unknown / unregistered names are errors.
    #[method(name = "search_provider_test")]
    async fn search_provider_test(
        &self,
        name: String,
        query: String,
    ) -> RpcResult<web::settings::ProviderProbe>;
    /// Per-provider health from the last calls (rate-limited, captcha, parse
    /// error, …) for every registered provider, in registration order.
    #[method(name = "search_status")]
    async fn search_status(&self) -> RpcResult<Vec<web::health::ProviderStatus>>;
    /// Run the whole `search_web` tool once (fan-out, rank, read) from Settings
    /// › Search "Run a test search"; returns the model-facing text and the JSON.
    #[method(name = "search_test")]
    async fn search_test(
        &self,
        query: String,
        site: Option<String>,
        recent: Option<web::Recency>,
    ) -> RpcResult<web::settings::SearchTestResult>;
}

#[async_trait]
impl SettingsApiServer for Server {

    async fn compact_style(&self) -> RpcResult<CompactStyle> {
        ask::<Agents, _>(agent::AGENT_ACTOR, GetCompactStyle).await
    }

    async fn set_compact_style(&self, style: CompactStyle) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetCompactStyle { style }).await
    }

    async fn set_subagent_timeout(&self, seconds: u64) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetSubagentTimeout { seconds }).await
    }

    async fn set_max_tool_rounds(&self, rounds: u32) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetMaxToolRounds { rounds }).await
    }

    async fn set_stream_idle(&self, seconds: u64) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetStreamIdle { seconds }).await
    }

    async fn set_max_salvages(&self, count: u32) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetMaxSalvages { count }).await
    }

    async fn set_pty_idle(&self, seconds: u64) -> RpcResult<()> {
        ask::<ToolRegistry, _>(tools::REGISTRY_ACTOR, SetPtyIdle { seconds }).await
    }

    async fn title_model(&self) -> RpcResult<Option<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, GetTitleModel).await
    }

    async fn set_title_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetTitleModel { model }).await
    }

    async fn title_effort(&self) -> RpcResult<Option<String>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::GetTitleEffort).await
    }

    async fn set_title_effort(&self, effort: Option<String>) -> RpcResult<Option<String>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::SetTitleEffort { effort }).await
    }

    async fn polish_model(&self) -> RpcResult<Option<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, GetPolishModel).await
    }

    async fn set_polish_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetPolishModel { model }).await
    }

    async fn polish_effort(&self) -> RpcResult<Option<String>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::GetPolishEffort).await
    }

    async fn set_polish_effort(&self, effort: Option<String>) -> RpcResult<Option<String>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::SetPolishEffort { effort }).await
    }

    async fn vision_model(&self) -> RpcResult<Option<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, GetVisionModel).await
    }

    async fn set_vision_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetVisionModel { model }).await
    }

    async fn vision_effort(&self) -> RpcResult<Option<String>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::GetVisionEffort).await
    }

    async fn set_vision_effort(&self, effort: Option<String>) -> RpcResult<Option<String>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::SetVisionEffort { effort }).await
    }

    async fn escalate_model(&self) -> RpcResult<EscalateModel> {
        ask::<Agents, _>(agent::AGENT_ACTOR, GetEscalateModel).await
    }

    async fn set_escalate_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetEscalateModel { model }).await
    }

    async fn escalate_effort(&self) -> RpcResult<Option<String>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::GetEscalateEffort).await
    }

    async fn set_escalate_effort(&self, effort: Option<String>) -> RpcResult<Option<String>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::SetEscalateEffort { effort }).await
    }

    async fn subagent_model(&self) -> RpcResult<Option<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, GetSubagentModel).await
    }

    async fn set_subagent_model(&self, model: Option<String>) -> RpcResult<Option<ModelInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetSubagentModel { model }).await
    }

    async fn list_models(
        &self,
        ext: &Extensions,
        engine: Option<String>,
    ) -> RpcResult<Vec<ModelInfo>> {
        // 183 W-12: another engine's catalog (New chat on a machine, the ⋯ model list).
        if let Some(r) =
            forward::to_engine(ext, engine.as_deref(), "list_models", Vec::<()>::new()).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, ListModels).await
    }

    async fn list_aws_profiles(&self) -> RpcResult<AwsProfiles> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListAwsProfiles).await
    }

    async fn set_aws_profile(&self, profile: Option<String>) -> RpcResult<AwsProfiles> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetAwsProfile { profile }).await
    }

    async fn set_model(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        model: String,
        manual: Option<bool>,
    ) -> RpcResult<ModelInfo> {
        self.authorize_opt_fwd(ext, session).await?;
        let manual = manual.unwrap_or(true);
        if let Some(s) = session
            && let Some(r) = forward::route(ext, s, "set_model", (session, &model, manual)).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SetModel {
                session,
                model,
                manual,
            },
        )
        .await
    }

    async fn set_model_favorite(&self, model: String, favorite: bool) -> RpcResult<Vec<String>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SetModelFavorite { model, favorite }).await
    }

    async fn set_effort(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        effort: Option<String>,
        manual: Option<bool>,
    ) -> RpcResult<()> {
        self.authorize_opt_fwd(ext, session).await?;
        let manual = manual.unwrap_or(true);
        if let Some(s) = session
            && let Some(r) =
                forward::route(ext, s, "set_effort", (session, &effort, manual)).await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SetEffort {
                session,
                effort,
                manual,
            },
        )
        .await
    }

    async fn set_advisor(
        &self,
        ext: &Extensions,
        session: SessionId,
        model: Option<String>,
        effort: Option<String>,
    ) -> RpcResult<AdvisorStatus> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SetAdvisor {
                session,
                model,
                effort,
            },
        )
        .await
    }

    async fn advisor_status(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<AdvisorStatus> {
        self.authorize(ext, session).await?;
        ask::<Agents, _>(agent::AGENT_ACTOR, AdvisorStatusOf { session }).await
    }

    async fn set_advisor_default_effort(
        &self,
        effort: Option<String>,
    ) -> RpcResult<AdvisorDefaults> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SetAdvisorDefaults {
                effort: Some(effort),
                ..SetAdvisorDefaults::default()
            },
        )
        .await
    }

    async fn set_advisor_notes_per_update(&self, n: u8) -> RpcResult<AdvisorDefaults> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SetAdvisorDefaults {
                max_notes_per_update: Some(n),
                ..SetAdvisorDefaults::default()
            },
        )
        .await
    }

    async fn set_advisor_immune_turns(&self, n: u8) -> RpcResult<AdvisorDefaults> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SetAdvisorDefaults {
                immune_turns: Some(n),
                ..SetAdvisorDefaults::default()
            },
        )
        .await
    }

    async fn set_advisor_sync_backlog(&self, v: SyncBacklog) -> RpcResult<AdvisorDefaults> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SetAdvisorDefaults {
                sync_backlog: Some(v),
                ..SetAdvisorDefaults::default()
            },
        )
        .await
    }

    async fn set_advisor_max_consults(&self, n: u8) -> RpcResult<AdvisorDefaults> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SetAdvisorDefaults {
                max_consults: Some(n),
                ..SetAdvisorDefaults::default()
            },
        )
        .await
    }

    async fn harness_options(
        &self,
        ext: &Extensions,
        session: SessionId,
    ) -> RpcResult<Vec<acp::ConfigOption>> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "harness_options", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, HarnessOptions { session }).await
    }

    async fn set_harness_option(
        &self,
        ext: &Extensions,
        session: SessionId,
        config_id: String,
        value: String,
    ) -> RpcResult<acp::SetOptionResult> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(
            ext,
            session,
            "set_harness_option",
            (session, &config_id, &value),
        )
        .await?
        {
            return Ok(r);
        }
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            SetHarnessOption {
                session,
                config_id,
                value,
            },
        )
        .await
    }

    async fn resume_child(&self, ext: &Extensions, session: SessionId) -> RpcResult<ChildResumed> {
        self.authorize_fwd(ext, session).await?;
        if let Some(r) = forward::route(ext, session, "resume_child", (session,)).await? {
            return Ok(r);
        }
        ask::<Agents, _>(agent::AGENT_ACTOR, ResumeChild { session }).await
    }

    async fn list_acp(&self, probe: bool) -> RpcResult<Vec<acp::EntryInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListAcp { probe }).await
    }

    async fn add_acp(&self, id: String) -> RpcResult<acp::EntryInfo> {
        ask::<Agents, _>(agent::AGENT_ACTOR, AddAcp { id }).await
    }

    async fn remove_acp(&self, name: String) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, RemoveAcp { name }).await
    }

    async fn list_acp_registry(&self) -> RpcResult<Vec<acp::registry::RegistryAgent>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListRegistry).await
    }

    async fn list_logins(&self) -> RpcResult<Vec<LoginInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListLogins).await
    }

    async fn login(&self, provider: String) -> RpcResult<String> {
        ask::<Agents, _>(agent::AGENT_ACTOR, Login { provider }).await
    }

    async fn logout(&self, provider: String) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, Logout { provider }).await
    }

    async fn list_providers(&self) -> RpcResult<Vec<ProviderInfo>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListProviders).await
    }

    async fn add_provider(
        &self,
        name: String,
        base_url: String,
        key: Option<String>,
        runtime: Option<agent::LocalRuntime>,
    ) -> RpcResult<ProviderInfo> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            AddProvider {
                name,
                base_url,
                key,
                runtime,
            },
        )
        .await
    }

    async fn remove_provider(&self, name: String) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, RemoveProvider { name }).await
    }

    async fn refresh_provider_models(&self, name: String) -> RpcResult<RefreshedModels> {
        ask::<Agents, _>(agent::AGENT_ACTOR, RefreshProviderModels { name }).await
    }

    async fn local_servers_probe(&self) -> RpcResult<Vec<agent::LocalServer>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::ProbeLocalServers).await
    }

    async fn provider_models(&self, provider: String) -> RpcResult<Vec<agent::ProviderModel>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::ProviderModels { provider }).await
    }

    async fn set_model_limits(
        &self,
        provider: String,
        model: String,
        window: Option<u32>,
        output: Option<u32>,
    ) -> RpcResult<agent::ProviderModel> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            agent::SetModelLimits {
                provider,
                model,
                context: window,
                output,
            },
        )
        .await
    }

    async fn test_summary_call(&self, model: String) -> RpcResult<agent::SummaryTest> {
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::TestSummaryCall { model }).await
    }

    async fn update_status(&self) -> RpcResult<Option<update::UpdateReady>> {
        send(&update_actor()?, update::Status).await
    }

    async fn endless_settings_get(&self) -> RpcResult<EndlessConfig> {
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::GetEndlessSettings).await
    }

    async fn endless_settings_set(&self, config: EndlessConfig) -> RpcResult<EndlessConfig> {
        ask::<Agents, _>(agent::AGENT_ACTOR, agent::SetEndlessSettings { config }).await
    }

    fn config_flag_get(&self, key: ConfigFlagKey) -> RpcResult<ConfigFlag> {
        Ok(self.config_flag(key))
    }

    fn config_flag_set(&self, key: ConfigFlagKey, on: bool) -> RpcResult<ConfigFlag> {
        let path = self.rt.data_dir.join("config.yaml");
        let (section, field) = key.path();
        runtime::write_config_value(&path, section, field, if on { "true" } else { "false" })
            .map_err(|e| failed(format!("config.yaml: {e}")))?;
        Ok(self.config_flag(key))
    }

    async fn search_provider_test(
        &self,
        name: String,
        query: String,
    ) -> RpcResult<web::settings::ProviderProbe> {
        ask::<web::WebActor, _>(web::WEB_ACTOR, web::settings::Probe { name, query }).await
    }

    async fn search_status(&self) -> RpcResult<Vec<web::health::ProviderStatus>> {
        ask::<web::WebActor, _>(web::WEB_ACTOR, web::settings::Status).await
    }

    async fn search_test(
        &self,
        query: String,
        site: Option<String>,
        recent: Option<web::Recency>,
    ) -> RpcResult<web::settings::SearchTestResult> {
        let out = ask::<ToolRegistry, _>(
            tools::REGISTRY_ACTOR,
            tools::Invoke(tools::ToolCall {
                id: format!("search-test-{}", uuid::Uuid::now_v7()),
                name: "search_web".into(),
                args: serde_json::json!({ "query": query, "site": site, "recent": recent }),
                session: SessionId::new(),
                quiet: true,
            }),
        )
        .await?;
        Ok(web::settings::SearchTestResult {
            text: out.content,
            is_error: out.is_error,
            structured: out.structured.unwrap_or(serde_json::Value::Null),
        })
    }
}
