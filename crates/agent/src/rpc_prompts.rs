//! System prompt section RPC methods: list, edit, and reset core areas and
//! registered plugin prompt blocks (`crate::prompt_sections`).

use crate::prompt_sections::{self, PromptSection};
use jsonrpsee::Extensions;
use jsonrpsee::core::{RpcResult, async_trait};
use jsonrpsee::proc_macros::rpc;
use rpc_core::{Server, ask, failed};
use tools::{ListPrompts, REGISTRY_ACTOR, ToolRegistry};

async fn plugin_prompts() -> RpcResult<Vec<(&'static str, String)>> {
    ask::<ToolRegistry, _>(REGISTRY_ACTOR, ListPrompts).await
}

/// System prompt section RPC methods owned by this plugin.
#[rpc_core_macros::rpc_descriptors(owner = "agent", descriptors = rpc_descriptors, audience = "owner")]
#[rpc(server, client)]
pub trait PromptsApi {
    /// Every editable system prompt section (core areas followed by registered plugin blocks),
    /// with its effective content, default content, and customization status.
    #[method(name = "list_prompt_sections", with_extensions)]
    async fn list_prompt_sections(&self) -> RpcResult<Vec<PromptSection>>;
    /// Override one system prompt section by `id` (or remove its override when `content` matches
    /// the default) and return the updated section.
    #[method(name = "set_prompt_section", with_extensions)]
    async fn set_prompt_section(&self, id: String, content: String) -> RpcResult<PromptSection>;
    /// Reset one system prompt section by `id` back to its built-in default and return the
    /// updated section.
    #[method(name = "reset_prompt_section", with_extensions)]
    async fn reset_prompt_section(&self, id: String) -> RpcResult<PromptSection>;
    /// Enable or disable a system prompt section without losing custom edits.
    #[method(name = "set_prompt_section_enabled", with_extensions)]
    async fn set_prompt_section_enabled(
        &self,
        id: String,
        enabled: bool,
    ) -> RpcResult<PromptSection>;
}

#[async_trait]
impl PromptsApiServer for Server {
    async fn list_prompt_sections(&self, ext: &Extensions) -> RpcResult<Vec<PromptSection>> {
        rpc_core::owner(ext)?;
        let plugins = plugin_prompts().await?;
        Ok(prompt_sections::list(&self.rt.data_dir, plugins))
    }

    async fn set_prompt_section(
        &self,
        ext: &Extensions,
        id: String,
        content: String,
    ) -> RpcResult<PromptSection> {
        rpc_core::owner(ext)?;
        let plugins = plugin_prompts().await?;
        prompt_sections::set(&self.rt.data_dir, plugins, &id, &content).map_err(failed)
    }

    async fn reset_prompt_section(
        &self,
        ext: &Extensions,
        id: String,
    ) -> RpcResult<PromptSection> {
        rpc_core::owner(ext)?;
        let plugins = plugin_prompts().await?;
        prompt_sections::reset(&self.rt.data_dir, plugins, &id).map_err(failed)
    }

    async fn set_prompt_section_enabled(
        &self,
        ext: &Extensions,
        id: String,
        enabled: bool,
    ) -> RpcResult<PromptSection> {
        rpc_core::owner(ext)?;
        let plugins = plugin_prompts().await?;
        prompt_sections::set_enabled(&self.rt.data_dir, plugins, &id, enabled).map_err(failed)
    }
}

pub(super) fn rpc_module(server: rpc_core::Server) -> rpc_core::DescribedModule {
    rpc_core::DescribedModule::new(PromptsApiServer::into_rpc(server), rpc_descriptors())
        .expect("agent RPC descriptors")
}
