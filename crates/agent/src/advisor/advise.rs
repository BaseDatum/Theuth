//! 125 §4.8 / §5.3 / §5.4: the loop-local `advise` tool — its spec, its
//! argument shape — and the model-facing `<advisory>` rendering of a card.

use serde::Deserialize;
use tools::{Risk, ToolSpec};

use crate::{AdvisorSeverity, AdvisoryNote};

/// Tool name.
pub const NAME: &str = "advise";

/// Tool description (omp's three lines joined).
const DESCRIPTION: &str = "Watched agent: send one concrete, terse piece of advice. Use sparingly; stay silent when nothing matters. Call to avert likely-wrong or materially wasteful work.";

/// `advise` arguments.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct AdviseArgs {
    /// One concrete piece of advice for the agent you are watching. Terse, specific, actionable.
    pub note: String,
    /// How strongly to weigh this. Omit for a plain nit.
    #[serde(default)]
    pub severity: Option<AdvisorSeverity>,
}

/// The direct tool spec appended for `kind == Advisor`.
pub fn spec() -> ToolSpec {
    ToolSpec {
        name: NAME.into(),
        description: DESCRIPTION.into(),
        schema: serde_json::to_value(schemars::schema_for!(AdviseArgs)).unwrap_or_default(),
        risk: Risk::ReadOnly,
        direct: true,
        returns: "str".into(),
    }
}

/// Parse the model's arguments; omitted severity is a nit.
pub fn parse(args: &serde_json::Value) -> Result<(String, AdvisorSeverity), String> {
    let a: AdviseArgs = serde_json::from_value(args.clone()).map_err(|e| e.to_string())?;
    Ok((a.note, a.severity.unwrap_or(AdvisorSeverity::Nit)))
}

/// Wire name of a severity.
pub fn severity_str(s: AdvisorSeverity) -> &'static str {
    match s {
        AdvisorSeverity::Nit => "nit",
        AdvisorSeverity::Concern => "concern",
        AdvisorSeverity::Blocker => "blocker",
    }
}

/// The body of an advisory is untrusted text: XML-escaped, and any `<`
/// opening an `advisory` tag neutralised (the `peer_wrapper` stance).
fn escape(body: &str) -> String {
    body.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// The user-row text of one card: one `<advisory>` element per note (125 §5.4).
pub fn card_text(notes: &[AdvisoryNote]) -> String {
    notes
        .iter()
        .map(|n| {
            format!(
                "<advisory severity=\"{}\" guidance=\"weigh, don't blindly obey\">\n{}\n</advisory>",
                severity_str(n.severity),
                escape(n.note.trim())
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_and_renders() {
        let (note, sev) = parse(&serde_json::json!({"note": "check it"})).unwrap();
        assert_eq!((note.as_str(), sev), ("check it", AdvisorSeverity::Nit));
        let (_, sev) = parse(&serde_json::json!({"note": "x", "severity": "blocker"})).unwrap();
        assert_eq!(sev, AdvisorSeverity::Blocker);
        assert!(parse(&serde_json::json!({})).is_err());
        let text = card_text(&[AdvisoryNote {
            severity: AdvisorSeverity::Concern,
            note: "a <b> & c".into(),
        }]);
        assert_eq!(
            text,
            "<advisory severity=\"concern\" guidance=\"weigh, don't blindly obey\">\na &lt;b&gt; &amp; c\n</advisory>"
        );
        assert_eq!(spec().name, NAME);
    }
}
