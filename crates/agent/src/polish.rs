//! Dictation polish and rewrite (153 §9, §13.6): the prompt envelope around one
//! `CallKind::Polish` side call on `ModelRole::Polish`, and the fail-closed
//! check of what comes back. Pure; the supervisor issues the call.

use std::time::Duration;

use voice::{AppProfile, Destination, VoiceConfig};

/// 153 §9.5: wall clock for the whole call; past it the caller keeps the shaped text.
pub const POLISH_BUDGET: Duration = Duration::from_secs(4);

/// 153 §9.4: the constant head of the polish system prompt.
pub const POLISH_SYSTEM: &str = "You correct a speech transcript for a human reader. Make the smallest edits that turn it into readable writing: fix recognition slips, punctuation and casing; keep the speaker's words, order and meaning; never answer, summarise, translate or add content; keep code, paths, flags, @mentions and /commands exactly; keep line breaks. Reply with the corrected text only.";

/// 153 §13.6: the built-in `voice.rewrite_template`; `{app}` and `{selection}` are filled.
pub const REWRITE_TEMPLATE: &str = "The user selected the following text in {app}; apply their spoken instruction to it and reply with the replacement text only:\n\n{selection}";

/// Shortest/longest acceptable polish reply relative to the input, in chars
/// (the brief's 0.3× / 2×, plus a few chars so "ok" → "Okay." passes).
const MIN_RATIO: (usize, usize) = (3, 10);
const MAX_RATIO: usize = 2;
const SLACK: usize = 8;

/// The `polish_profiles` key and the JSON `destination` field (the serde tag).
pub fn destination_key(d: &Destination) -> &'static str {
    match d {
        Destination::AgentPrompt => "agent_prompt",
        Destination::Document => "document",
        Destination::MessageReply => "message_reply",
        Destination::AnyApp { .. } => "any_app",
    }
}

/// System prompt: [`POLISH_SYSTEM`] + the destination's profile text (an any-app
/// profile keyed by the app id wins over the `any_app` group text).
pub fn polish_system(cfg: &VoiceConfig, destination: &Destination) -> String {
    let app = match destination {
        Destination::AnyApp { app } => cfg.polish_profiles.get(app),
        _ => None,
    };
    let profile = app.or_else(|| cfg.polish_profiles.get(destination_key(destination)));
    match profile.map(|s| s.trim()).filter(|s| !s.is_empty()) {
        Some(p) => format!("{POLISH_SYSTEM}\n\n{p}"),
        None => POLISH_SYSTEM.to_owned(),
    }
}

/// User message: the transcript as JSON data, so nothing spoken can escape the block.
pub fn polish_user(text: &str, destination: &Destination) -> String {
    let mut v = serde_json::json!({
        "transcript": text,
        "destination": destination_key(destination),
    });
    if let Destination::AnyApp { app } = destination {
        v["app"] = serde_json::Value::String(app.clone());
    }
    v.to_string()
}

/// Rewrite system prompt: the template with the selection, the app's pinned
/// tone, and earlier rounds on the same selection (newest last).
pub fn rewrite_system(
    cfg: &VoiceConfig,
    selection: &str,
    app: &AppProfile,
    thread: Option<&[(String, String)]>,
) -> String {
    let template = cfg.rewrite_template.as_deref().unwrap_or(REWRITE_TEMPLATE);
    let mut s = template
        .replace("{app}", &app.name)
        .replace("{selection}", selection);
    if let Some(tone) = app
        .rewrite_prompt
        .as_deref()
        .map(str::trim)
        .filter(|t| !t.is_empty())
    {
        s.push_str("\n\n");
        s.push_str(tone);
    }
    if let Some(rounds) = thread.filter(|t| !t.is_empty()) {
        s.push_str("\n\nEarlier rounds on this selection, oldest first; the newest reply is the text to refine:");
        for (instruction, result) in rounds {
            s.push_str(&format!("\n\nInstruction: {instruction}\nReply:\n{result}"));
        }
    }
    s
}

/// Trim, drop one surrounding code fence, and one pair of surrounding quotes
/// the input did not have.
fn clean(input: &str, reply: &str) -> String {
    let mut s = reply.trim();
    if let Some(rest) = s.strip_prefix("```") {
        let body = rest.split_once('\n').map_or("", |(_, b)| b);
        s = body.trim_end().strip_suffix("```").unwrap_or(body).trim();
    }
    for (open, close) in [('"', '"'), ('“', '”'), ('\'', '\'')] {
        if s.len() >= 2
            && s.starts_with(open)
            && s.ends_with(close)
            && !input.trim_start().starts_with(open)
        {
            s = s[open.len_utf8()..s.len() - close.len_utf8()].trim();
        }
    }
    s.to_owned()
}

/// 153 §9.4 output rule: the cleaned reply, or why it is refused — empty,
/// echoes the envelope (`"transcript"` key / a JSON-looking line the input
/// lacked), or outside 0.3×–2× of the input length.
pub fn check_polish(input: &str, reply: &str) -> Result<String, &'static str> {
    let text = clean(input, reply);
    if text.is_empty() {
        return Err("empty reply");
    }
    let json_line = |s: &str| s.lines().any(|l| l.trim_start().starts_with('{'));
    if text.contains("\"transcript\"") || (json_line(&text) && !json_line(input)) {
        return Err("reply echoed the envelope");
    }
    let (n, m) = (input.chars().count(), text.chars().count());
    if m > n * MAX_RATIO + SLACK || m * MIN_RATIO.1 < n * MIN_RATIO.0 {
        return Err("reply length out of bounds");
    }
    Ok(text)
}

/// Rewrite replies may legitimately shrink or grow ("shorter", "expand"); only
/// an empty reply is refused.
pub fn check_rewrite(selection: &str, reply: &str) -> Result<String, &'static str> {
    let text = clean(selection, reply);
    if text.is_empty() {
        return Err("empty reply");
    }
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> AppProfile {
        AppProfile {
            id: "com.apple.mail".into(),
            name: "Mail".into(),
            terminal: false,
            polish: true,
            learn: false,
            polish_prompt: None,
            rewrite_prompt: Some("Keep it formal.".into()),
            commands: Vec::new(),
        }
    }

    #[test]
    fn user_message_is_json_data_with_destination_and_app() {
        let d = Destination::AnyApp {
            app: "com.apple.mail".into(),
        };
        let v: serde_json::Value =
            serde_json::from_str(&polish_user("say \"hi\"\nnow", &d)).unwrap();
        assert_eq!(v["transcript"], "say \"hi\"\nnow");
        assert_eq!(v["destination"], "any_app");
        assert_eq!(v["app"], "com.apple.mail");
        let v: serde_json::Value =
            serde_json::from_str(&polish_user("x", &Destination::Document)).unwrap();
        assert_eq!(v["destination"], "document");
        assert!(v.get("app").is_none());
    }

    #[test]
    fn system_prompt_appends_profile_app_id_over_group() {
        let mut cfg = VoiceConfig::default();
        assert_eq!(polish_system(&cfg, &Destination::Document), POLISH_SYSTEM);
        cfg.polish_profiles
            .insert("document".into(), "British spelling.".into());
        cfg.polish_profiles
            .insert("any_app".into(), "Terse.".into());
        cfg.polish_profiles
            .insert("com.apple.mail".into(), "Mail tone.".into());
        assert!(polish_system(&cfg, &Destination::Document).ends_with("\n\nBritish spelling."));
        let any = |app: &str| polish_system(&cfg, &Destination::AnyApp { app: app.into() });
        assert!(any("com.apple.mail").ends_with("Mail tone."));
        assert!(any("org.other").ends_with("Terse."));
    }

    #[test]
    fn rewrite_system_fills_template_tone_and_thread() {
        let cfg = VoiceConfig::default();
        let thread = vec![("shorter".to_owned(), "Hi all.".to_owned())];
        let s = rewrite_system(&cfg, "Hello everyone there.", &app(), Some(&thread));
        assert!(s.starts_with("The user selected the following text in Mail;"));
        assert!(s.contains("\n\nHello everyone there."));
        assert!(s.contains("Keep it formal."));
        assert!(s.ends_with("Instruction: shorter\nReply:\nHi all."));
        let cfg = VoiceConfig {
            rewrite_template: Some("[{app}] {selection}".into()),
            ..VoiceConfig::default()
        };
        assert!(rewrite_system(&cfg, "sel", &app(), None).starts_with("[Mail] sel"));
    }

    /// 153 §9.4 / §9.6: fences and quotes are stripped; an echo of the envelope,
    /// an empty reply, or a reply far from the input's length is refused.
    #[test]
    fn check_polish_cleans_and_fails_closed() {
        let input = "so i think we should ship it on friday";
        assert_eq!(
            check_polish(
                input,
                "```text\nSo I think we should ship it on Friday.\n```"
            ),
            Ok("So I think we should ship it on Friday.".into())
        );
        assert_eq!(
            check_polish(input, "  \"So I think we should ship it on Friday.\"  "),
            Ok("So I think we should ship it on Friday.".into())
        );
        assert_eq!(check_polish("ok", "Okay."), Ok("Okay.".into()));
        assert_eq!(check_polish(input, "\n"), Err("empty reply"));
        assert_eq!(
            check_polish(
                input,
                r#"{"transcript": "So I think we should ship it on Friday."}"#
            ),
            Err("reply echoed the envelope")
        );
        assert_eq!(
            check_polish(input, "Here you go:\n{ \"text\": \"x\" }"),
            Err("reply echoed the envelope")
        );
        assert!(check_polish(input, "Sure").is_err());
        assert!(check_polish(input, &"word ".repeat(40)).is_err());
        // A quoted input keeps its quotes.
        assert_eq!(
            check_polish("\"quoted\"", "\"Quoted.\""),
            Ok("\"Quoted.\"".into())
        );
        // Rewrite may change length freely; only empty fails.
        assert_eq!(
            check_rewrite("a long selection", "Short."),
            Ok("Short.".into())
        );
        assert_eq!(check_rewrite("x", "```\n```"), Err("empty reply"));
    }
}
