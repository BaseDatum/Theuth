//! Built-in Workspace/Business example Deck (`Workspace Pulse (Example)`).
//!
//! Demonstrates:
//! - Collaborative sections (native `json-render`, strict-CSP `html`, and `note`).
//! - Scheduled collection & persistent streaming (`on_start` + cooperative `on_step` + `deck_sleep` + `deck_checkpoint`).
//! - Declared interactive actions (`on_action` with JSON Schema args, per-caller private results, explicit shared emits).
//! - Explicit synthetic-data labelling (never fakes connected external integrations).
//! - Review-gated script proposal (`pending_review` until the owner explicitly approves & starts).

use document::Principal;
use document::deck::{CreateDeck, DeckOp, SectionKind};
use serde_json::json;

use crate::model::{DeckDetail, DeclaredAction, ScriptMode};
use crate::owner::DeckSupervisor;

/// Portable Monty script for `Workspace Pulse (Example)`.
/// All numeric data is synthetic example data; `inspect_tools` queries the real session `tools()`.
pub const WORKSPACE_PULSE_SCRIPT: &str = r#"# Workspace Pulse (Example) — Portable Monty Deck Script
# NOTE: Metrics below are synthetic example data unless you wire real MCP/session tools.

tick: int = 0
arr_k: float = 482.5
open_p1: int = 3
velocity: int = 44
pipeline_deals: int = 18
closed_today: int = 2
history: list[int] = [42, 44, 43, 45, 44]
last_event: str = "Example workspace initialized (synthetic data)"

def _state_dict() -> dict:
    return {
        "tick": tick,
        "arr_k": arr_k,
        "open_p1": open_p1,
        "velocity": velocity,
        "pipeline_deals": pipeline_deals,
        "closed_today": closed_today,
        "history": list(history),
        "last_event": last_event,
    }

def _build_payload(source_label: str) -> dict:
    return {
        "data_label": "Example Workspace Data (Synthetic — no live SaaS connected)",
        "tick": tick,
        "arr_display": f"${arr_k:.1f}k",
        "arr_k": arr_k,
        "open_p1": str(open_p1),
        "open_p1_num": open_p1,
        "velocity": f"{velocity} pts",
        "velocity_num": velocity,
        "pipeline_deals": str(pipeline_deals),
        "closed_today": str(closed_today),
        "history": list(history),
        "last_event": last_event,
        "source": source_label,
    }

async def on_start(last_state):
    global tick, arr_k, open_p1, velocity, pipeline_deals, closed_today, history, last_event
    if isinstance(last_state, dict) and "tick" in last_state:
        tick = int(last_state.get("tick", 0))
        arr_k = float(last_state.get("arr_k", 482.5))
        open_p1 = int(last_state.get("open_p1", 3))
        velocity = int(last_state.get("velocity", 44))
        pipeline_deals = int(last_state.get("pipeline_deals", 18))
        closed_today = int(last_state.get("closed_today", 2))
        raw_hist = last_state.get("history")
        if isinstance(raw_hist, list):
            history = [int(x) for x in raw_hist]
        last_event = f"Restored from checkpoint at tick {tick} (example data)"
    deck_emit(_build_payload("on_start"), durable=True)
    deck_checkpoint(_state_dict())

async def on_step():
    global tick, arr_k, velocity, history, last_event
    tick = tick + 1
    # Deterministic gentle drift so reviewers see live updates cleanly without randomness.
    delta_arr = 1.5 if (tick % 3 == 0) else 0.4
    arr_k = round(arr_k + delta_arr, 1)
    v_delta = 1 if (tick % 2 == 0) else -1
    velocity = max(25, min(65, velocity + v_delta))
    history = list(history)
    history.append(velocity)
    if len(history) > 16:
        history = history[-16:]
    last_event = f"Streaming heartbeat #{tick} (synthetic example data)"

    payload = _build_payload("on_step")
    # Transient live frame on every tick; durable snapshot every 5 ticks.
    deck_emit(payload, durable=(tick % 5 == 0))
    if tick % 5 == 0:
        deck_checkpoint(_state_dict())
    deck_sleep(3.0)

async def on_action(action, args, ctx):
    global tick, arr_k, open_p1, velocity, pipeline_deals, closed_today, history, last_event
    if action == "triage_incident":
        severity = str(args.get("severity", "P1"))
        note = str(args.get("note", "Assigned to on-call"))
        if open_p1 > 0:
            open_p1 = open_p1 - 1
        last_event = f"Triaged {severity} incident: {note} (example)"
        deck_checkpoint(_state_dict())
        deck_emit(_build_payload("action:triage_incident"), durable=True)
        return {
            "status": "triaged",
            "severity": severity,
            "open_p1_remaining": open_p1,
            "note": note,
            "caller": ctx.get("caller_id", "unknown"),
        }

    if action == "record_deal":
        amount_k = float(args.get("amount_k", 12.0))
        account = str(args.get("account", "Example Corp"))
        arr_k = round(arr_k + amount_k, 1)
        closed_today = closed_today + 1
        if pipeline_deals > 0:
            pipeline_deals = pipeline_deals - 1
        last_event = f"Recorded example deal: {account} (+${amount_k:.1f}k)"
        deck_checkpoint(_state_dict())
        deck_emit(_build_payload("action:record_deal"), durable=True)
        return {
            "status": "recorded",
            "account": account,
            "new_arr_k": arr_k,
            "closed_today": closed_today,
        }

    if action == "inspect_tools":
        # Private to caller — does NOT call deck_emit so other viewers never see it.
        available = tools()
        return {
            "private_notice": "Real tools exposed to this Deck's isolated SessionId",
            "tool_count": len(available),
            "sample_tools": available[:12],
        }

    if action == "reset_metrics":
        tick = 0
        arr_k = 482.5
        open_p1 = 3
        velocity = 44
        pipeline_deals = 18
        closed_today = 2
        history = [42, 44, 43, 45, 44]
        last_event = "Reset synthetic example metrics"
        deck_checkpoint(_state_dict())
        deck_emit(_build_payload("action:reset_metrics"), durable=True)
        return {"status": "reset", "arr_k": arr_k}

    raise ValueError(f"Unsupported action: {action}")
"#;

pub fn workspace_pulse_declared_actions() -> Vec<DeclaredAction> {
    vec![
        DeclaredAction {
            name: "triage_incident".into(),
            title: "Triage P1 Incident".into(),
            description: "Decrement open P1 count and record a triage note (example state).".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "severity": { "type": "string", "enum": ["P0", "P1", "P2"] },
                    "note": { "type": "string", "minLength": 1, "maxLength": 200 }
                },
                "additionalProperties": false
            }),
            mutates_state: true,
            allow_while_paused: true,
        },
        DeclaredAction {
            name: "record_deal".into(),
            title: "Record Example Deal".into(),
            description: "Add a synthetic closed-won deal to example ARR.".into(),
            schema: json!({
                "type": "object",
                "properties": {
                    "account": { "type": "string", "minLength": 1, "maxLength": 120 },
                    "amount_k": { "type": "number", "minimum": 0.5, "maximum": 5000 }
                },
                "required": ["account", "amount_k"],
                "additionalProperties": false
            }),
            mutates_state: true,
            allow_while_paused: true,
        },
        DeclaredAction {
            name: "inspect_tools".into(),
            title: "Inspect Session Tools".into(),
            description:
                "Privately list real session/MCP tools available to this Deck (no shared emit)."
                    .into(),
            schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            mutates_state: false,
            allow_while_paused: true,
        },
        DeclaredAction {
            name: "reset_metrics".into(),
            title: "Reset Example Metrics".into(),
            description: "Reset synthetic example workspace metrics to defaults.".into(),
            schema: json!({
                "type": "object",
                "properties": {},
                "additionalProperties": false
            }),
            mutates_state: true,
            allow_while_paused: true,
        },
    ]
}

/// Native `json-render` section template using `{{key}}` live snapshot/frame interpolation.
pub fn workspace_pulse_surface_json() -> String {
    serde_json::to_string_pretty(&json!({
        "root": "root",
        "elements": {
            "root": {
                "type": "Stack",
                "props": { "direction": "vertical", "gap": "md" },
                "children": ["banner", "metrics", "controls", "actions", "result"]
            },
            "banner": {
                "type": "Alert",
                "props": {
                    "type": "info",
                    "title": "{{data_label|Example Workspace Data (Synthetic — approve script to stream)}}",
                    "message": "Latest event: {{last_event|Pending script approval — click Approve & Start above}} (tick {{tick|0}})"
                }
            },
            "metrics": {
                "type": "MetricGrid",
                "props": {
                    "items": [
                        { "label": "ARR Run Rate (Example)", "value": "{{arr_display|$482.5k}}", "delta": "+{{closed_today|2}} today", "trend": "up" },
                        { "label": "Open P1 Incidents", "value": "{{open_p1|3}}", "trend": "flat" },
                        { "label": "Sprint Velocity", "value": "{{velocity|44 pts}}", "trend": "up" },
                        { "label": "Active Pipeline", "value": "{{pipeline_deals|18}}", "trend": "flat" }
                    ]
                }
            },
            "controls": {
                "type": "StreamControls",
                "props": {
                    "title": "Workspace Pulse Runtime",
                    "show_run_once": true
                }
            },
            "actions": {
                "type": "Stack",
                "props": { "direction": "horizontal", "gap": "sm" },
                "children": ["btn_triage", "btn_deal", "btn_tools", "btn_reset"]
            },
            "btn_triage": {
                "type": "ActionButton",
                "props": {
                    "action": "triage_incident",
                    "label": "Triage P1",
                    "args_json": "{\"severity\":\"P1\",\"note\":\"Assigned via Deck button\"}",
                    "tone": "primary"
                }
            },
            "btn_deal": {
                "type": "ActionButton",
                "props": {
                    "action": "record_deal",
                    "label": "+$15k Example Deal",
                    "args_json": "{\"account\":\"Acme Example Co\",\"amount_k\":15}",
                    "tone": "default"
                }
            },
            "btn_tools": {
                "type": "ActionButton",
                "props": {
                    "action": "inspect_tools",
                    "label": "Inspect Session Tools (Private)",
                    "args_json": "{}",
                    "tone": "default"
                }
            },
            "btn_reset": {
                "type": "ActionButton",
                "props": {
                    "action": "reset_metrics",
                    "label": "Reset",
                    "args_json": "{}",
                    "tone": "danger"
                }
            },
            "result": {
                "type": "ResultBanner",
                "props": {
                    "title": "Private Action Result"
                }
            }
        }
    }))
    .expect("valid example surface json")
}

/// Strictly sandboxed HTML section (zero network/forms/same-origin) consuming the exact same
/// live snapshot/frame via `window.theuthDeck`.
pub const WORKSPACE_PULSE_HTML: &str = r#"<!doctype html>
<html>
<head>
<meta charset="utf-8" />
<style>
  :root { color-scheme: light dark; font-family: system-ui, -apple-system, sans-serif; }
  body { margin: 0; padding: 14px; background: transparent; color: #18181b; }
  @media (prefers-color-scheme: dark) { body { color: #f4f4f5; } }
  .card { border: 1px solid rgba(128,128,128,0.25); border-radius: 10px; padding: 12px; }
  .top { display: flex; justify-content: space-between; align-items: center; gap: 8px; margin-bottom: 10px; }
  .badge { font-size: 11px; padding: 2px 8px; border-radius: 999px; background: rgba(59,130,246,0.14); color: #2563eb; }
  .bars { display: flex; align-items: flex-end; gap: 4px; height: 76px; padding: 8px 4px; border-radius: 8px; background: rgba(128,128,128,0.08); }
  .bar { flex: 1; background: linear-gradient(180deg, #3b82f6, #6366f1); border-radius: 4px 4px 0 0; min-height: 6px; transition: height 180ms ease; }
  .foot { display: flex; justify-content: space-between; align-items: center; margin-top: 10px; font-size: 12px; opacity: 0.8; }
  button { cursor: pointer; border: 1px solid rgba(128,128,128,0.35); background: rgba(59,130,246,0.12); color: inherit; border-radius: 6px; padding: 4px 10px; font-size: 12px; }
  button:hover { background: rgba(59,130,246,0.22); }
</style>
</head>
<body>
  <div class="card">
    <div class="top">
      <strong style="font-size:13px;">Velocity Pulse (Sandboxed HTML)</strong>
      <span class="badge" id="src">Example Data</span>
    </div>
    <div class="bars" id="bars"></div>
    <div class="foot">
      <span id="meta">Waiting for live snapshot…</span>
      <button id="triage-btn" type="button">Quick Triage P1</button>
    </div>
  </div>
  <script>
    const barsEl = document.getElementById('bars');
    const metaEl = document.getElementById('meta');
    const srcEl = document.getElementById('src');

    function render(data) {
      const hist = Array.isArray(data && data.history) ? data.history : [42, 44, 43, 45, 44];
      const max = Math.max(1, ...hist);
      const bars = hist.map((v) => {
        const num = Number(v);
        const pct = Math.max(12, Math.min(100, Math.round(((Number.isFinite(num) ? num : 0) / max) * 100)));
        const el = document.createElement('div');
        el.className = 'bar';
        el.style.height = `${pct}%`;
        el.title = `${String(v)} pts`;
        return el;
      });
      barsEl.replaceChildren(...bars);
      const tick = data && data.tick != null ? data.tick : 0;
      const p1 = data && data.open_p1 != null ? data.open_p1 : '3';
      metaEl.textContent = `Tick #${tick} · Open P1: ${p1}`;
      srcEl.textContent = (data && data.source) ? `Example (${data.source})` : 'Example Data';
    }

    render(null);

    if (window.theuthDeck) {
      window.theuthDeck.onData(function(payload) {
        render(payload && payload.data ? payload.data : payload);
      });
      document.getElementById('triage-btn').addEventListener('click', function() {
        window.theuthDeck.action('triage_incident', { severity: 'P1', note: 'Triaged from HTML pulse widget' });
      });
    }
  </script>
</body>
</html>
"#;

pub const WORKSPACE_PULSE_NOTE: &str = "\
### Operating Notes (Collaborative CRDT Section)

- **Example Data Notice**: Metrics in this Deck are **synthetic example data** so you can explore live streaming, interactive actions, and whiteboard embedding safely before connecting real tools.
- **Independent Edits**: Edit this note section freely — it lives in its own `Y.Map` section with per-section revision history (`expected_rev` CAS) and never overwrites the KPI layout or triggers script re-approval.
- **Connect Real Data**: Ask your agent *\"Connect this Deck to my real workspace tools\"* — the agent will interview you, inspect available MCP/session tools, and propose an updated script for your explicit approval.
";

/// Create a complete `Workspace Pulse (Example)` Deck with collaborative sections and a
/// `pending_review` portable Monty script.
pub async fn create_workspace_pulse_example(
    sup: &mut DeckSupervisor,
    by: Principal,
) -> Result<DeckDetail, String> {
    let created = sup
        .shared()
        .document
        .ask(CreateDeck {
            title: Some("Workspace Pulse (Example)".into()),
            description: Some(
                "Example Workspace & Operations Deck demonstrating collaborative sections, persistent Monty streaming, interactive actions, and strict-CSP HTML."
                    .into(),
            ),
            by: by.clone(),
        })
        .send()
        .await
        .map_err(|e| format!("{e}"))?;

    let deck_id = created.doc.as_str().to_owned();
    let rec = sup
        .shared()
        .store
        .ensure_deck(&deck_id)
        .await
        .map_err(|e| format!("{e:#}"))?;
    sup.shared().register_session(&deck_id, rec.session_id);

    // 1. Native json-render KPI + Controls + Actions section
    sup.doc_op(
        &deck_id,
        by.clone(),
        DeckOp::SectionAdd {
            kind: SectionKind::JsonRender,
            title: "Executive Pulse & Actions".into(),
            content: workspace_pulse_surface_json(),
            col_span: Some(7),
            data_key: None,
            index: None,
        },
    )
    .await
    .map_err(|e| e.to_string())?;

    // 2. Strict-CSP Sandboxed HTML section consuming the same live snapshot/frame
    sup.doc_op(
        &deck_id,
        by.clone(),
        DeckOp::SectionAdd {
            kind: SectionKind::Html,
            title: "Velocity Sparkline (Strict HTML)".into(),
            content: WORKSPACE_PULSE_HTML.to_owned(),
            col_span: Some(5),
            data_key: None,
            index: None,
        },
    )
    .await
    .map_err(|e| e.to_string())?;

    // 3. Collaborative markdown Note section
    sup.doc_op(
        &deck_id,
        by.clone(),
        DeckOp::SectionAdd {
            kind: SectionKind::Note,
            title: "Team Operating Notes".into(),
            content: WORKSPACE_PULSE_NOTE.to_owned(),
            col_span: Some(12),
            data_key: None,
            index: None,
        },
    )
    .await
    .map_err(|e| e.to_string())?;

    // 4. Propose portable Monty script (ALWAYS pending_review until owner approves).
    let _ = sup
        .shared()
        .store
        .propose_script(
            &deck_id,
            WORKSPACE_PULSE_SCRIPT.to_owned(),
            workspace_pulse_declared_actions(),
            ScriptMode::Hybrid,
            Some(3),
            None,
            "example_template".into(),
            "Initial Workspace Pulse (Example) script with synthetic data, streaming, and declared actions".into(),
        )
        .await
        .map_err(|e| format!("{e:#}"))?;

    // Mirror read-only pointer into Y.Doc script_refs.
    let _ = sup
        .doc_op(
            &deck_id,
            Principal::Helper,
            DeckOp::UpdateScriptRef {
                script_ref: document::deck::DeckScriptRef {
                    active_rev: None,
                    pending_rev: Some(1),
                    mode: Some("hybrid".into()),
                    schedule: Some("every 3s".into()),
                    updated_at: chrono::Utc::now().timestamp_millis(),
                },
            },
        )
        .await;

    sup.shared()
        .runtime
        .publish(crate::model::DeckChanged {
            deck_id: deck_id.clone(),
            title: created.title,
            updated_ms: chrono::Utc::now().timestamp_millis(),
            deleted: false,
        })
        .await;

    sup.build_detail(&deck_id, by).await
}
