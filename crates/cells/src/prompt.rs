//! Owner: A1 — the `## User cells` prompt block (design 171 §10.1) and the bundled `cells`
//! skill (§10.2–10.5). Both empty until A1 lands: an empty block is never registered and an
//! empty skill is never written, so installed prompt text stays byte-identical.

use serde::{Deserialize, Serialize};

/// The `RegisterPrompt{plugin: "cells"}` text.
pub const PROMPT: &str = "";

/// The bundled skill, `<data_dir>/skills/cells.md`.
pub const SKILL: &str = include_str!("../cells.md");

/// A1's keys of the `cells:` section (flattened into [`crate::CellsConfig`]).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PromptConfig {}
