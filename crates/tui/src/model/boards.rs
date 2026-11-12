use super::*;

impl Model {

    /// The board (design doc 19 §3.3): the subtree's live rows plus errors
    /// and cancels the picker has not shown yet, in picker order. Empty =
    /// hidden.
    pub fn board(&self) -> Vec<&AgentRow> {
        let mut rows: Vec<&AgentRow> = self.agents.values().filter(|r| r.on_board()).collect();
        rows.sort_by_key(|r| rank(r));
        rows
    }
}

/// `/tasks`: `id · state · owner · title`, oldest first as the engine lists them (67 §12).
/// The heading names the project, or `all projects` when the session has none.
pub fn tasks_table(project: Option<&str>, rows: &[tasks::Task]) -> String {
    let scope = project.unwrap_or("all projects");
    if rows.is_empty() {
        return format!("no tasks ({scope})");
    }
    let mut out = vec![
        format!("tasks · {scope}"),
        "id · state · owner · title".to_owned(),
    ];
    out.extend(rows.iter().map(|t| {
        let id = &t.id.0;
        let short = &id[id.len().saturating_sub(6)..];
        format!("{short} · {} · {} · {}", t.state.as_str(), t.owner, t.title)
    }));
    out.join("\n")
}

/// `/roster`: `name · status · open` per roster agent (67 §12; `/agents` is the subagent picker).
pub fn agents_table(rows: &[roster::AgentSummary]) -> String {
    if rows.is_empty() {
        return "no agents".into();
    }
    let mut out = vec!["name · status · open".to_owned()];
    out.extend(rows.iter().map(|a| {
        let status = a
            .paused_label()
            .or_else(|| a.waiting_label())
            .unwrap_or_else(|| {
                serde_json::to_value(a.status)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default()
            });
        format!("{} · {status} · {}", a.file.name.as_str(), a.open_tasks)
    }));
    out.join("\n")
}
