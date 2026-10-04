//! The `decide` / `decide_many` runtime functions (design doc 92): the classifier as a tool the
//! reasoning model programs. This commit: the family predicate the escalation guard needs.

use crate::TOOL_FAMILY;

/// `tool` or `tool.<task>` (92 §2): the families the `decide` tool asks under. They have no
/// tier 2 ([`crate::escalate`] refuses them).
pub fn is_tool_family(family: &str) -> bool {
    family == TOOL_FAMILY
        || family
            .strip_prefix(TOOL_FAMILY)
            .is_some_and(|rest| rest.starts_with('.'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_family_predicate() {
        assert!(is_tool_family("tool"));
        assert!(is_tool_family("tool.mail"));
        assert!(!is_tool_family("tools"));
        assert!(!is_tool_family("toolbox.x"));
        assert!(!is_tool_family("meeting.kind"));
    }
}
