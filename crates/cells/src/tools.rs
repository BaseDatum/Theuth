//! Owner: A1 — the three read-only runtime functions (design 171 §9.5): `cell_runs`,
//! `cell_output`, `interpreters` (`Risk::ReadOnly`), and the prompt block registration.
//! Invariant 2: nothing registered here starts, stops, types into or restarts a kernel —
//! the agent reads runs, it never makes one.

use std::sync::Arc;

use runtime::Runtime;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tools::{DynTool, Risk, Tool, ToolCx, ToolOutput};

use crate::{CellRef, CellRun, Cells, CellsError, DocId, ExitStatus, RunId, Scope};

/// Register the runtime functions and the `## User cells` prompt block once the tool
/// registry runs (171 §10.6; the `app` plugin is the precedent).
pub async fn register(rt: &Runtime, cells: Cells) {
    let rt = rt.clone();
    tokio::spawn(async move {
        let Some(reg) = rt
            .lookup_when::<tools::ToolRegistry>(tools::REGISTRY_ACTOR)
            .await
        else {
            return;
        };
        for tool in functions(cells) {
            let _ = reg.ask(tools::RegisterTool(tool)).await;
        }
        let _ = reg
            .ask(tools::RegisterPrompt {
                plugin: crate::PLUGIN,
                text: crate::prompt::PROMPT.to_owned(),
            })
            .await;
    });
}

/// Everything the cells plugin registers with the agent: three readers, nothing else.
fn functions(cells: Cells) -> Vec<DynTool> {
    vec![
        Arc::new(CellRunsTool(cells.clone())),
        Arc::new(CellOutputTool(cells.clone())),
        Arc::new(InterpretersTool(cells)),
    ]
}

/// A refusal, as the model reads it.
fn refused(name: &str, e: CellsError) -> anyhow::Error {
    anyhow::anyhow!("{name}: {e}")
}

/// Arguments for [`CellRunsTool`].
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CellRunsArgs {
    /// `this` (this session's chat cells, the default) or a note's document id.
    #[serde(default)]
    pub scope: Option<String>,
    /// Only runs of the note cell with this `name`.
    #[serde(default)]
    pub name: Option<String>,
    /// Newest first; default 20.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `cell_runs`: the runs the user started from this session's cells or a note's.
pub struct CellRunsTool(Cells);

#[async_trait::async_trait]
impl Tool for CellRunsTool {
    const NAME: &'static str = "cell_runs";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "list[dict]";
    type Args = CellRunsArgs;

    fn description(&self) -> String {
        "Runs of user cells, newest first: scope \"this\" (this chat's cells, default) or a note's doc id. \
         Returns [{run, name, cell, state, exit, secs, at, expect, summary, edited, private, pressed, ran_past}] \
         (ran_past: the user ran it past an unarmed `after:` gate, and why; else null). Reads only: \
         the user runs cells, you never do."
            .into()
    }

    async fn call(&self, a: CellRunsArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let scope = match a.scope.as_deref() {
            None | Some("this") => Scope::Session(cx.session),
            Some(doc) => Scope::Doc(DocId(doc.to_owned())),
        };
        let runs = self
            .0
            .runs(scope, a.limit.unwrap_or(20))
            .await
            .map_err(|e| refused(Self::NAME, e))?;
        let rows = Value::Array(
            runs.iter()
                .filter(|r| a.name.is_none() || r.name == a.name)
                .map(run_row)
                .collect(),
        );
        let text = rows.to_string();
        Ok(ToolOutput {
            structured: Some(rows),
            ..ToolOutput::ok(text)
        })
    }
}

/// One `cell_runs` row (§9.5).
fn run_row(r: &CellRun) -> Value {
    let exit = r.exit.as_ref().map(|e| match e {
        ExitStatus::Code(c) => json!(c),
        ExitStatus::Signal(s) => json!(s),
    });
    let expect = (!r.expect.is_empty()).then(|| {
        let met = r.expect.iter().filter(|e| e.met).count();
        let n = r.expect.len();
        if met == n {
            format!("met {met}/{n}")
        } else {
            format!("missed {}/{n}", n - met)
        }
    });
    let at = r.ended_ms.or(r.started_ms).unwrap_or(r.queued_ms);
    let at = chrono::DateTime::from_timestamp_millis(at).map(|t| {
        t.with_timezone(&chrono::Local)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    });
    json!({
        "run": r.id.0,
        "name": r.name,
        "cell": r.cell,
        "state": r.state,
        "exit": exit,
        "secs": r.secs,
        "at": at,
        "expect": expect,
        "summary": r.summary,
        "edited": r.edited,
        "private": r.private,
        "pressed": r.pressed_by,
        "ran_past": r.ran_past,
    })
}

/// Arguments for [`CellOutputTool`].
#[derive(Debug, Deserialize, JsonSchema)]
pub struct CellOutputArgs {
    /// The run id (`r_…`, the `run` attribute of a `<cell_result>`).
    pub run: String,
    /// Byte offset into the cleaned text; default 0.
    #[serde(default)]
    pub offset: Option<u64>,
    /// Bytes to read; default 8000.
    #[serde(default)]
    pub limit: Option<u32>,
}

/// `cell_output`: a page of a run's cleaned, vault-redacted, credential-masked text.
pub struct CellOutputTool(Cells);

#[async_trait::async_trait]
impl Tool for CellOutputTool {
    const NAME: &'static str = "cell_output";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "dict";
    type Args = CellOutputArgs;

    fn description(&self) -> String {
        "A page of a user cell run's output (cleaned terminal text, secrets masked): the part a \
         <cell_result> cut. Returns {text, offset, total_bytes, next} (next = null at the end). \
         Refused for private cells and for another chat's runs."
            .into()
    }

    async fn call(&self, a: CellOutputArgs, cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let run = RunId(a.run);
        // Scope rule (§9.5): this session's chat runs and note runs; never another chat's.
        let record = self
            .0
            .run_get(run.clone())
            .await
            .map_err(|e| refused(Self::NAME, e))?;
        if let CellRef::Chat { session, .. } = record.run.cell
            && session != cx.session
        {
            return Err(refused(Self::NAME, CellsError::NotFound(run.0)));
        }
        let page = self
            .0
            .output(run, a.offset.unwrap_or(0), a.limit.unwrap_or(8000), false)
            .await
            .map_err(|e| refused(Self::NAME, e))?;
        let data = json!({
            "text": &page.text,
            "offset": page.offset,
            "total_bytes": page.total,
            "next": page.next,
        });
        Ok(ToolOutput {
            structured: Some(data),
            ..ToolOutput::ok(page.text)
        })
    }
}

/// Arguments for [`InterpretersTool`].
#[derive(Debug, Deserialize, JsonSchema)]
pub struct InterpretersArgs {
    /// The folder a cell would run in (absolute or `~/…`).
    pub cwd: String,
}

/// `interpreters`: what can run a cell in `cwd` and what `auto` picks (§6.4).
pub struct InterpretersTool(Cells);

#[async_trait::async_trait]
impl Tool for InterpretersTool {
    const NAME: &'static str = "interpreters";
    const RISK: Risk = Risk::ReadOnly;
    const RETURNS: &'static str = "list[dict]";
    type Args = InterpretersArgs;

    fn description(&self) -> String {
        "Interpreters and shells a user cell in `cwd` can use, as the user's cell chip lists them: \
         [{kind, source, path, version, why, auto}] — `auto: true` is what `#| python: auto` (or a \
         shell cell) picks there."
            .into()
    }

    async fn call(&self, a: InterpretersArgs, _cx: ToolCx) -> anyhow::Result<ToolOutput> {
        let list = self
            .0
            .interpreters(a.cwd)
            .await
            .map_err(|e| refused(Self::NAME, e))?;
        let data = serde_json::to_value(&list)?;
        let text = data.to_string();
        Ok(ToolOutput {
            structured: Some(data),
            ..ToolOutput::ok(text)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::NoteKey;

    /// §9.5 / invariant 2: the plugin registers exactly the three readers, each read-only
    /// and runtime-only (never a direct tool), each with an object schema; nothing that
    /// could start, stop, type into or restart a kernel.
    #[tokio::test]
    async fn only_three_read_only_runtime_functions() {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let cells = crate::actor::start(&rt, crate::CellsConfig::default()).await;
        let specs: Vec<tools::ToolSpec> = functions(cells).iter().map(|t| t.spec()).collect();
        let names: Vec<&str> = specs.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, ["cell_runs", "cell_output", "interpreters"]);
        for s in &specs {
            assert_eq!(s.risk, Risk::ReadOnly, "{}", s.name);
            assert!(!s.direct, "{}", s.name);
            assert_eq!(s.schema["type"], "object", "{}", s.name);
        }
        let props = |i: usize| specs[i].schema["properties"].as_object().unwrap().len();
        assert_eq!((props(0), props(1), props(2)), (3, 3, 1));
    }

    /// §9.5: every key of a `cell_runs` row is present; `name` and `private` come from the
    /// record (L11).
    #[test]
    fn cell_runs_rows_carry_every_key() {
        let doc = DocId("d1".into());
        let run = CellRun {
            id: RunId("r_1".into()),
            cell: CellRef::Note {
                doc: doc.clone(),
                key: NoteKey::Name("check-lag".into()),
            },
            mode: crate::RunMode::Cell,
            pass: None,
            kernel: crate::KernelId {
                scope: Scope::Doc(doc),
                spec: crate::KernelSpec::Monty,
                instance: 0,
            },
            interpreter: "Monty".into(),
            cwd: "/tmp".into(),
            env_names: Vec::new(),
            presented_sha: String::new(),
            code_sha: String::new(),
            edited: false,
            pressed_by: "desktop".into(),
            state: crate::RunState::Done,
            exit: Some(ExitStatus::Code(0)),
            secs: Some(1.5),
            expect: Vec::new(),
            summary: None,
            output_bytes: None,
            queued_ms: 0,
            started_ms: None,
            ended_ms: None,
            masked: 0,
            private: true,
            name: Some("check-lag".into()),
            input: None,
            ran_past: Some("ran past after: preview — it had not run".into()),
        };
        let row = run_row(&run);
        let keys: Vec<&str> = row
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        for k in [
            "run", "name", "cell", "state", "exit", "secs", "at", "expect", "summary", "edited",
            "private", "pressed", "ran_past",
        ] {
            assert!(keys.contains(&k), "{k}");
        }
        assert_eq!(
            (
                &row["name"],
                &row["private"],
                &row["exit"],
                &row["ran_past"]
            ),
            (
                &json!("check-lag"),
                &json!(true),
                &json!(0),
                &json!("ran past after: preview — it had not run")
            )
        );
    }
}
