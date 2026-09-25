//! 67 §10: a chat message addressed to an agent by name. The first token,
//! lowercased, with one trailing `:` or `,` stripped, must be a roster name
//! exactly (`opsy` is not `ops`). Callers add `theuth` to `names` so
//! `theuth: …` routes to the default agent explicitly.

/// Where the message goes and what is left of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Route<'a> {
    /// The agent named (as listed in `names`).
    pub agent: String,
    /// The text after the name, trimmed.
    pub rest: &'a str,
}

/// `Ops: check prod` with `names = ["ops"]` → `ops` / `check prod`. `None`
/// when the first token is not a name.
pub fn route<'a>(text: &'a str, names: &[String]) -> Option<Route<'a>> {
    let text = text.trim_start();
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    let (token, rest) = text.split_at(end);
    let token = token.to_lowercase();
    let token = token
        .strip_suffix(':')
        .or_else(|| token.strip_suffix(','))
        .unwrap_or(&token);
    let agent = names.iter().find(|n| n.as_str() == token)?;
    Some(Route {
        agent: agent.clone(),
        rest: rest.trim(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<String> {
        vec!["ops".into(), "theuth".into()]
    }

    #[test]
    fn colon_comma_case_and_rest() {
        let r = route("Ops: check prod", &names()).unwrap();
        assert_eq!((r.agent.as_str(), r.rest), ("ops", "check prod"));
        let r = route("ops, please check prod", &names()).unwrap();
        assert_eq!((r.agent.as_str(), r.rest), ("ops", "please check prod"));
        let r = route("OPS", &names()).unwrap();
        assert_eq!((r.agent.as_str(), r.rest), ("ops", ""));
    }

    #[test]
    fn prefix_is_not_a_name() {
        assert_eq!(route("opsy check prod", &names()), None);
        assert_eq!(route("check prod ops", &names()), None);
        assert_eq!(route("", &names()), None);
    }

    #[test]
    fn theuth_routes_explicitly() {
        let r = route("theuth what is up", &names()).unwrap();
        assert_eq!((r.agent.as_str(), r.rest), ("theuth", "what is up"));
    }
}
