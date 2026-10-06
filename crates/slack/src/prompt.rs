//! The `## Slack` prompt section (design doc 110 §7.2), registered by
//! `SlackPlugin::start` as `RegisterPrompt { plugin: "slack" }`. Wave 2
//! (§8) swaps the monospace-box clause for "which Slack shows as native blocks".

/// One paragraph after its heading, like `## Voice notes`.
pub const PROMPT: &str = "## Slack\n\
A user message that starts with `[slack dm]` or `[slack channel]` was typed in Slack, and your reply \
renders there as a Slack message in the same thread. Lead with the answer. One screen — about twelve \
lines — unless the user asks for more. Write the markdown Slack renders: bold, bullets, numbered lists, \
tables of up to six columns, fenced code, links. Draw nothing Slack cannot show: no mermaid, no openui, \
and no json-render except Alert, Callout, Table, Bars, Sparkline, Metric, MetricGrid, KeyValueList and \
Checklist — and until Slack shows those natively they arrive as a monospace box, so prefer a markdown \
table. Anything longer or richer than one screen goes through `show`; it arrives as a file the user \
can preview. Your tool activity is one collapsed card the user can expand, so the sentence before a \
batch of tool calls says what you are doing and why in one line, and you never list what each call \
did. Subagents appear to the user as their own threads and a live board; do not narrate their \
progress. `message_user` reaches other channels; to answer here, just reply. Keep this style while \
messages carry the `[slack …]` marker; go back to your normal style when they stop.";

#[cfg(test)]
mod tests {
    use super::PROMPT;

    #[test]
    fn prompt_section_starts_with_the_heading_and_names_every_native_block() {
        assert!(PROMPT.starts_with("## Slack\n"));
        let (heading, body) = PROMPT.split_once('\n').unwrap();
        assert_eq!(heading, "## Slack");
        assert!(!body.contains('\n'), "one paragraph after the heading");
        for c in [
            "Alert",
            "Callout",
            "Table",
            "Bars",
            "Sparkline",
            "Metric",
            "MetricGrid",
            "KeyValueList",
            "Checklist",
        ] {
            assert!(body.contains(c), "{c} missing");
        }
        assert!(body.contains("`[slack dm]`") && body.contains("`[slack channel]`"));
    }
}
