//! 67 §6.5: an agent proposing a hire files a task with its manager; the
//! task is the approval. Pure: the actor asks the tasks plugin to create it.

use crate::types::AgentFile;

/// The `Hire agent …` task: title, the proposed file's facts, the charter,
/// and how to approve or refuse.
pub fn hire_task(file: &AgentFile, proposed_by: &str, manager: tasks::Owner) -> tasks::TaskCreate {
    let name = file.name.as_str();
    let mut brief = format!(
        "{proposed_by} proposes hiring **{}** (`{name}`).\n\n\
         - reports_to: {}\n- budget: {}\n- tools: {}\n- mcp: {}\n\n## Charter\n\n{}\n\n",
        file.title,
        file.reports_to,
        file.budget,
        file.tools.as_deref().unwrap_or("default"),
        if file.mcp.is_empty() { "all".to_owned() } else { file.mcp.join(", ") },
        file.body.trim(),
    );
    brief.push_str(&format!(
        "Approve with `agent_approve` (Agents view / RPC); refuse by deleting `agents/{name}.md`."
    ));
    tasks::TaskCreate {
        title: format!("Hire agent {name} ({})", file.title),
        brief,
        owner: manager,
        reviewer: None,
        parent: None,
        project: None,
        where_: None,
        harness: None,
        external: vec![],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file;

    #[test]
    fn hire_task_names_the_agent_the_charter_and_the_approval() {
        let f = file::parse(
            "ops",
            "---\ntitle: Ops\nreports_to: cto\nbudget: 40 USD/month\ntools: mutating\nmcp: [grafana]\n---\nYou run prod.\n",
        )
        .unwrap();
        let t = hire_task(&f, "agent:cto", tasks::Owner::Agent("cto".into()));
        assert_eq!(t.title, "Hire agent ops (Ops)");
        assert_eq!(t.owner, tasks::Owner::Agent("cto".into()));
        assert!(t.brief.starts_with("agent:cto proposes hiring **Ops** (`ops`)."));
        assert!(t.brief.contains("- reports_to: cto\n- budget: 40 USD/month\n- tools: mutating\n- mcp: grafana\n"));
        assert!(t.brief.contains("## Charter\n\nYou run prod.\n"));
        assert!(t.brief.ends_with("Approve with `agent_approve` (Agents view / RPC); refuse by deleting `agents/ops.md`."));
    }
}
