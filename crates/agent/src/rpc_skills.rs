use crate as agent;
use crate::rpc_forward as forward;
use crate::rpc_support::{RpcServerExt, owner_root};
#[cfg(not(target_os = "ios"))]
use crate::{AddSkillRepo, RemoveSkillRepo, SyncSkills};
use crate::{AddSkill, Agents, ListSkills, LoadSkill, RemoveSkill, SkillBody, SkillMeta, UseSkill};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::{Server, ask};
use runtime::SessionId;
use std::path::PathBuf;
#[cfg(not(target_os = "ios"))]
use tools::git::SyncResult;

#[rpc_core_macros::rpc_descriptors(owner = "agent", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait SkillsApi {
    /// Every skill under `<data_dir>/skills`.
    #[method(name = "list_skills")]
    async fn list_skills(&self) -> RpcResult<Vec<SkillMeta>>;
    /// Body (and supporting files) of one skill.
    #[method(name = "load_skill")]
    async fn load_skill(&self, name: String) -> RpcResult<SkillBody>;
    /// Hand a skill to the model as user input: its body wrapped in
    /// `<skill name="…">`, then `args` (design doc 04 §2.4). New turn when
    /// idle, steer when running; `None` session starts one. `files` are
    /// attached to that input exactly as `send_input`'s (image files, 17 §2);
    /// optional, absent = none.
    #[method(name = "use_skill", with_extensions)]
    async fn use_skill(
        &self,
        session: Option<SessionId>,
        name: String,
        args: String,
        files: Option<Vec<PathBuf>>,
    ) -> RpcResult<SessionId>;
    /// [`use_skill`](Self::use_skill) for several skills at once: one input
    /// whose `<skill>` blocks follow `names`' order, then `args`, carrying
    /// `files` like `send_input`'s.
    #[method(name = "use_skills", with_extensions)]
    async fn use_skills(
        &self,
        session: Option<SessionId>,
        names: Vec<String>,
        args: String,
        files: Option<Vec<PathBuf>>,
    ) -> RpcResult<SessionId>;
    /// Write `skills/<name>.md`.
    #[method(name = "add_skill")]
    async fn add_skill(
        &self,
        name: String,
        description: String,
        body: String,
        always: bool,
    ) -> RpcResult<SkillMeta>;
    /// Delete a local skill.
    #[method(name = "remove_skill")]
    async fn remove_skill(&self, name: String) -> RpcResult<()>;
    /// Clone a skill repo into `skills/<name>`; returns the skills it holds.
    #[cfg(not(target_os = "ios"))]
    #[method(name = "add_skill_repo")]
    async fn add_skill_repo(&self, url: String, name: Option<String>) -> RpcResult<Vec<SkillMeta>>;
    /// Delete a cloned skill repo.
    #[cfg(not(target_os = "ios"))]
    #[method(name = "remove_skill_repo")]
    async fn remove_skill_repo(&self, name: String) -> RpcResult<()>;
    /// `git pull` every skill repo.
    #[cfg(not(target_os = "ios"))]
    #[method(name = "sync_skills")]
    async fn sync_skills(&self) -> RpcResult<Vec<SyncResult>>;
}

#[async_trait]
impl SkillsApiServer for Server {
    async fn list_skills(&self) -> RpcResult<Vec<SkillMeta>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, ListSkills).await
    }

    async fn load_skill(&self, name: String) -> RpcResult<SkillBody> {
        ask::<Agents, _>(agent::AGENT_ACTOR, LoadSkill { name }).await
    }

    async fn use_skill(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        name: String,
        args: String,
        files: Option<Vec<PathBuf>>,
    ) -> RpcResult<SessionId> {
        self.authorize_opt_fwd(ext, session).await?;
        let session = match session {
            Some(s) => s,
            None => owner_root().await?,
        };
        let files = files.unwrap_or_default();
        // Without files the forward keeps the three-parameter shape an older owner takes.
        let routed = if files.is_empty() {
            forward::route(ext, session, "use_skill", (Some(session), &name, &args)).await?
        } else {
            let params = (Some(session), &name, &args, forward::ByName(&files));
            forward::route(ext, session, "use_skill", params).await?
        };
        if let Some(r) = routed {
            return Ok(r);
        }
        let files = forward::resolve_files(ext, &self.rt.data_dir, files).await?;
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            UseSkill {
                session: Some(session),
                names: vec![name],
                args,
                files,
            },
        )
        .await
    }

    async fn use_skills(
        &self,
        ext: &Extensions,
        session: Option<SessionId>,
        names: Vec<String>,
        args: String,
        files: Option<Vec<PathBuf>>,
    ) -> RpcResult<SessionId> {
        self.authorize_opt_fwd(ext, session).await?;
        let session = match session {
            Some(s) => s,
            None => owner_root().await?,
        };
        let files = files.unwrap_or_default();
        let routed = if files.is_empty() {
            forward::route(ext, session, "use_skills", (Some(session), &names, &args)).await?
        } else {
            let params = (Some(session), &names, &args, forward::ByName(&files));
            forward::route(ext, session, "use_skills", params).await?
        };
        if let Some(r) = routed {
            return Ok(r);
        }
        let files = forward::resolve_files(ext, &self.rt.data_dir, files).await?;
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            UseSkill {
                session: Some(session),
                names,
                args,
                files,
            },
        )
        .await
    }

    async fn add_skill(
        &self,
        name: String,
        description: String,
        body: String,
        always: bool,
    ) -> RpcResult<SkillMeta> {
        ask::<Agents, _>(
            agent::AGENT_ACTOR,
            AddSkill {
                name,
                description,
                body,
                always,
            },
        )
        .await
    }

    async fn remove_skill(&self, name: String) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, RemoveSkill { name }).await
    }

    #[cfg(not(target_os = "ios"))]
    async fn add_skill_repo(&self, url: String, name: Option<String>) -> RpcResult<Vec<SkillMeta>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, AddSkillRepo { url, name }).await
    }

    #[cfg(not(target_os = "ios"))]
    async fn remove_skill_repo(&self, name: String) -> RpcResult<()> {
        ask::<Agents, _>(agent::AGENT_ACTOR, RemoveSkillRepo { name }).await
    }

    #[cfg(not(target_os = "ios"))]
    async fn sync_skills(&self) -> RpcResult<Vec<SyncResult>> {
        ask::<Agents, _>(agent::AGENT_ACTOR, SyncSkills).await
    }
}

/// Build this domain's RPC module.
pub fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(SkillsApiServer::into_rpc(server), rpc_descriptors())
        .expect("agent RPC descriptors")
}
