//! User cells (design doc 171 CELLS): code the user runs with one press in chat replies,
//! notes and notebooks. A plugin crate: its own sqlite under `<data_dir>/cells/`, one actor,
//! kernels (shell PTY jobs, a python shim, Monty), a run store, `<cell_result>` delivery, RPC.
//! The grammar is `surface::cell`.
//!
//! Module owners (docs/design/171-ownership.md):
//! - C (frozen): [`types`], [`error`], [`config`], [`kernel`], this file, `migrations/0001`.
//! - E1: [`actor`] (method set frozen), [`shell`], [`run`], [`render`], [`redact`], [`store`],
//!   [`deliver`].
//! - A1: [`prompt`], [`tools`], `cells.md`.
//! - E2: [`python`], [`monty`], [`interp`], [`note`], `shim.py`.

use runtime::Runtime;

pub mod actor;
pub mod config;
pub mod deliver;
pub mod error;
pub mod interp;
pub mod kernel;
pub mod monty;
pub mod note;
mod notes;
pub mod prompt;
pub mod python;
pub mod redact;
pub mod render;
pub mod run;
pub mod shell;
pub mod store;
pub mod tools;
pub mod types;
#[cfg(test)]
mod rpc_test_support;
/// Kernel RPC server and client traits.
#[allow(clippy::too_many_arguments)]
pub mod rpc;
mod kernel_rpc;
mod kernel_latest;
pub use rpc::rpc_module;

pub use actor::{CELLS_ACTOR, Cells, CellsActor};
pub use config::CellsConfig;
pub use error::CellsError;
pub use types::*;

/// `Plugin::name`, the config section and the prompt block's key.
pub const PLUGIN: &str = "cells";
/// `<data_dir>/cells/`.
pub const DIR: &str = "cells";
/// Where the bundled skill is written, root-relative (A1; only when [`prompt::SKILL`] is non-empty).
pub const SKILL_PATH: &str = "skills/cells.md";

/// Declare wire contracts before starting producers, even when disabled.
pub async fn declare_wire(rt: &runtime::Runtime) -> anyhow::Result<()> {
    use wire::RuntimeWire as _;
    rt.register_wire_event::<CellEvent>().await
}

/// The plugin. Reads `cells:`; when enabled spawns the (idle) [`CellsActor`] and hands its
/// handle to the A1 registration line.
pub struct CellsPlugin;

#[async_trait::async_trait]
impl runtime::Plugin for CellsPlugin {
    fn name(&self) -> &'static str {
        PLUGIN
    }

    async fn start(self: Box<Self>, rt: &Runtime) -> anyhow::Result<()> {
        declare_wire(rt).await?;
        let cfg = CellsConfig::from_runtime(rt);
        if !cfg.enabled {
            tracing::info!("cells disabled by config; no actor");
            return Ok(());
        }
        let cells = actor::start(rt, cfg).await; // 171:E1
        note::install(&cells); // 171:E2 (`document::set_run_annotations`)
        tools::register(rt, cells).await; // 171:A1
        Ok(())
    }
}
