//! One score for the inbox and an agent's wake dispatch (132 D4): the
//! project's place in `tasks.project_order` dominates, the user's priority
//! breaks ties. Higher sooner.

use crate::types::Priority;

/// One project step; larger than any priority weight so order beats priority.
pub const RANK_STEP: u32 = 100;

/// `len - index` for a project in `order`, 0 for an unranked or absent project.
pub fn project_score(project: Option<&str>, order: &[String]) -> u32 {
    project
        .and_then(|p| order.iter().position(|o| o == p))
        .map_or(0, |i| (order.len() - i) as u32)
}

/// `project_score * RANK_STEP + priority.weight()`; higher sooner.
pub fn rank(priority: Priority, project: Option<&str>, order: &[String]) -> u32 {
    project_score(project, order) * RANK_STEP + priority.weight()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn order() -> Vec<String> {
        vec!["sre".into(), "web".into()]
    }

    #[test]
    fn unranked_is_zero_and_ranked_beats_it() {
        assert_eq!(project_score(None, &order()), 0);
        assert_eq!(project_score(Some("docs"), &order()), 0);
        assert_eq!(project_score(Some("sre"), &order()), 2);
        assert_eq!(project_score(Some("web"), &order()), 1);
        assert!(
            rank(Priority::Low, Some("web"), &order()) > rank(Priority::Urgent, None, &order())
        );
    }

    #[test]
    fn project_step_dominates_priority() {
        assert!(
            rank(Priority::Low, Some("sre"), &order())
                > rank(Priority::Urgent, Some("web"), &order())
        );
        assert_eq!(rank(Priority::Medium, Some("sre"), &order()), 205);
    }

    #[test]
    fn weights() {
        assert_eq!(rank(Priority::Urgent, None, &[]), 9);
        assert_eq!(rank(Priority::High, None, &[]), 7);
        assert_eq!(rank(Priority::Medium, None, &[]), 5);
        assert_eq!(rank(Priority::Low, None, &[]), 2);
        assert_eq!(Priority::parse("high"), Some(Priority::High));
        assert_eq!(Priority::parse("x"), None);
        assert_eq!(Priority::default().as_str(), "medium");
    }
}
