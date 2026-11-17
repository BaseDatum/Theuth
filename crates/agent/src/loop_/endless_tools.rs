//! 137 §9: the three direct tools of an endless session — `zoom`, `recall`,
//! `date`. Specs are added to the turn's sorted list only when the turn runs
//! the endless frame; the handlers call the `Endless` service and return
//! ordinary tool results (capped and paged like any other by `finish_batch`).

use runtime::SessionId;
use serde::Deserialize;
use tools::{Risk, ToolOutput, ToolSpec};

use crate::endless::{Endless, ViewLine};

pub const ZOOM: &str = "zoom";
pub const RECALL: &str = "recall";
pub const DATE: &str = "date";

/// Whether `name` is one of the endless tools.
pub fn is_local(name: &str) -> bool {
    matches!(name, ZOOM | RECALL | DATE)
}

#[derive(Deserialize, schemars::JsonSchema)]
struct ZoomArgs {
    /// First message of the line (`id` in `id+n`).
    id: u64,
    /// Messages the line covers (`n` in `id+n`); a power of two.
    n: u64,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct RecallArgs {
    /// Words to search for.
    query: String,
    /// Lines to return (default 8).
    #[serde(default)]
    limit: Option<u8>,
}

#[derive(Deserialize, schemars::JsonSchema)]
struct DateArgs {
    /// The message id.
    id: u64,
}

/// The three specs (unsorted; the caller sorts the whole list).
pub fn specs() -> Vec<ToolSpec> {
    let spec = |name: &str, description: &str, schema: serde_json::Value| ToolSpec {
        name: name.into(),
        description: description.into(),
        schema,
        risk: Risk::ReadOnly,
        direct: true,
        returns: "str".to_owned(),
    };
    vec![
        spec(
            ZOOM,
            "Open the line id+n of the view into the two lines of n/2 under it; n = 1 gives the message whole.",
            serde_json::to_value(schemars::schema_for!(ZoomArgs)).unwrap_or_default(),
        ),
        spec(
            RECALL,
            "Search the one-line summaries of every past message for words; returns id+1 lines you can zoom. Use it when the view only hints at something, or for an exact error, path, number or name.",
            serde_json::to_value(schemars::schema_for!(RecallArgs)).unwrap_or_default(),
        ),
        spec(
            DATE,
            "The date and time of message id.",
            serde_json::to_value(schemars::schema_for!(DateArgs)).unwrap_or_default(),
        ),
    ]
}

/// Run one endless tool call. `name` is one of [`is_local`]'s.
pub async fn run(
    endless: &Endless,
    session: SessionId,
    name: &str,
    args: &serde_json::Value,
) -> ToolOutput {
    match name {
        ZOOM => match serde_json::from_value::<ZoomArgs>(args.clone()) {
            Ok(a) => match endless.zoom(session, a.id, a.n).await {
                Ok(lines) => ToolOutput::ok(render(&lines)),
                Err(e) => ToolOutput::err(e),
            },
            Err(e) => ToolOutput::err(format!("zoom: bad arguments: {e}")),
        },
        RECALL => match serde_json::from_value::<RecallArgs>(args.clone()) {
            Ok(a) => {
                let limit = a.limit.unwrap_or(8).max(1);
                match endless.recall(session, a.query.trim(), limit).await {
                    Ok(lines) if lines.is_empty() => ToolOutput::ok("No lines match.".to_owned()),
                    Ok(lines) => ToolOutput::ok(render(&lines)),
                    Err(e) => ToolOutput::err(e),
                }
            }
            Err(e) => ToolOutput::err(format!("recall: bad arguments: {e}")),
        },
        DATE => match serde_json::from_value::<DateArgs>(args.clone()) {
            Ok(a) => match endless.date(session, a.id).await {
                Ok(Some(at)) => ToolOutput::ok(
                    at.with_timezone(&chrono::Local)
                        .to_rfc3339_opts(chrono::SecondsFormat::Secs, false),
                ),
                Ok(None) => ToolOutput::ok(format!("No message {}.", a.id)),
                Err(e) => ToolOutput::err(e),
            },
            Err(e) => ToolOutput::err(format!("date: bad arguments: {e}")),
        },
        other => ToolOutput::err(format!("{other} is not an autocontext tool")),
    }
}

/// `id+n|text` per line, newlines of a whole unit (`n == 1`) kept.
pub fn render(lines: &[ViewLine]) -> String {
    lines
        .iter()
        .map(|l| format!("{}+{}|{}", l.id, l.n, l.text))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn specs_are_the_three_tools_with_verbatim_descriptions() {
        let s = specs();
        let names: Vec<&str> = s.iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, [ZOOM, RECALL, DATE]);
        assert!(s.iter().all(|t| t.direct && t.risk == Risk::ReadOnly));
        assert!(
            s[0].description
                .starts_with("Open the line id+n of the view")
        );
        assert!(
            s[1].description
                .starts_with("Search the one-line summaries")
        );
        assert_eq!(s[2].description, "The date and time of message id.");
        assert!(s[0].schema["properties"]["id"].is_object());
        assert!(s[1].schema["properties"]["limit"].is_object());
    }

    #[test]
    fn render_keeps_unit_newlines() {
        let lines = vec![ViewLine {
            id: 4,
            n: 1,
            level: 0,
            text: "user: a\nb".into(),
            built: true,
            at: None,
            mechanical: false,
        }];
        assert_eq!(render(&lines), "4+1|user: a\nb");
        assert!(is_local(ZOOM) && is_local(RECALL) && is_local(DATE) && !is_local("todo"));
    }
}
