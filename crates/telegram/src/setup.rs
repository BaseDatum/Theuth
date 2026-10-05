//! `t --telegram-setup` (design doc 98 §10.4): bot token → vault, command
//! scopes, pairing by deep link, `telegram.owner` + `enabled` in
//! `config.yaml`. Every step is re-runnable; a token already in the vault
//! that passes `getMe` and an owner already set are kept with a line saying
//! so. The only code in the crate that talks to Telegram outside the daemon;
//! tests drive it through [`run_with_host`] against `wiremock`.

use std::io::Write;
use std::path::Path;

use anyhow::{Context as _, bail};
use tokio::time::Instant;

use crate::api::{Api, CommandScope, Sender as _};
use crate::poll::{Incoming, backoff_delay, classify};
use crate::{BotIdentity, PAIR_TTL, SECRET_TOKEN, commands, mint_code};

/// The interactive terminal, behind a trait for tests.
pub trait Terminal {
    /// Asks `q`, returns the trimmed line typed.
    fn prompt(&mut self, q: &str) -> anyhow::Result<String>;
    /// Asks `q` for the bot token; echoes (it is pasted, not typed — slack's rule).
    fn secret(&mut self, q: &str) -> anyhow::Result<String>;
    /// Prints a line.
    fn say(&mut self, s: &str);
}

/// Stdin / stdout.
pub struct Tty;

impl Terminal for Tty {
    fn prompt(&mut self, q: &str) -> anyhow::Result<String> {
        print!("{q}");
        std::io::stdout().flush()?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            bail!("stdin closed");
        }
        Ok(line.trim().to_owned())
    }

    fn secret(&mut self, q: &str) -> anyhow::Result<String> {
        self.prompt(q)
    }

    fn say(&mut self, s: &str) {
        println!("{s}");
    }
}

/// What `t --telegram-setup` prints when an engine holds `engine.lock`.
pub const DAEMON_RUNNING: &str = "daemon running: use /telegram link or the telegram_link tool";

/// `t --telegram-setup`: token → vault, command scopes, pairing, config.
pub async fn run(data_dir: &Path, term: &mut dyn Terminal) -> anyhow::Result<()> {
    run_with_host(data_dir, term, None).await
}

/// [`run`] with the Bot API host overridden (`wiremock` in tests).
pub async fn run_with_host(
    data_dir: &Path,
    term: &mut dyn Terminal,
    host: Option<&str>,
) -> anyhow::Result<()> {
    term.say("theuth telegram setup");
    // A running engine owns the token and the poller (63 §2.1): one poller per token.
    let Some(_lock) = engine_lock(data_dir)? else {
        term.say(DAEMON_RUNNING);
        return Ok(());
    };
    let config = data_dir.join("config.yaml");
    let vault = open_vault(data_dir, &config).await?;

    // 1. Token, verified with getMe.
    let existing = vault.get(SECRET_TOKEN, 0);
    let mut fresh = None;
    let (api, me) = match existing.as_deref().map(|t| verify(t, host)) {
        Some(ok) => match ok.await {
            Ok(v) => {
                term.say("1. bot token ok (existing)");
                v
            }
            Err(e) => {
                term.say(&format!("1. the token in the vault no longer works: {e:#}"));
                let (tok, v) = ask_token(term, host).await?;
                fresh = Some(tok);
                v
            }
        },
        None => {
            let (tok, v) = ask_token(term, host).await?;
            fresh = Some(tok);
            v
        }
    };
    term.say(&format!(
        "   @{} · topics {}",
        me.username,
        if me.topics { "on" } else { "off" }
    ));
    if !me.topics {
        term.say(
            "   turn Topics on so each session gets its own topic (98 §6): @BotFather → /mybots → the bot → Bot Settings → Topics",
        );
    }

    // 2. Vault.
    match fresh {
        Some(tok) => {
            vault
                .set(SECRET_TOKEN, &tok, Some("Telegram bot token"), "telegram setup")
                .await
                .map_err(anyhow::Error::msg)?;
            term.say(&format!("2. token saved to the vault as {SECRET_TOKEN}"));
        }
        None => term.say("2. token already in the vault"),
    }

    // 3. Command scopes (the owner's chat scope follows pairing).
    for (scope, cmds) in commands::scopes(None) {
        api.set_commands(scope, &cmds).await?;
    }
    term.say("3. commands registered");

    // 4. Pairing.
    let owner = match current_owner(&config) {
        Some(o)
            if !term
                .prompt(&format!("4. owner {o} already set; re-pair? [y/N]: "))?
                .eq_ignore_ascii_case("y") =>
        {
            term.say(&format!("   owner {o} kept"));
            o
        }
        _ => {
            let code = mint_code();
            let url = deep_link(&me.username, &code);
            term.say(&format!(
                "4. open this from the owner's Telegram and press Start (valid {} min):\n{url}\n{}",
                PAIR_TTL.as_secs() / 60,
                link::qr(&url)
            ));
            let (user, name) = wait_for_start(&api, &me, &code)
                .await?
                .context("no /start with this code arrived in time; run the setup again")?;
            term.say(&format!("   paired with {name} ({user})"));
            user
        }
    };

    // 5. Config and the owner's command scope.
    runtime::write_config_value(&config, "telegram", "owner", &owner.to_string())?;
    runtime::write_config_value(&config, "telegram", "enabled", "true")?;
    if let Ok(chat) = i64::try_from(owner) {
        for (scope, cmds) in commands::scopes(Some(chat)) {
            if matches!(scope, CommandScope::Chat(_)) {
                api.set_commands(scope, &cmds).await?;
            }
        }
    }
    term.say(&format!("5. telegram.owner = {owner}, telegram.enabled = true"));
    term.say(
        "groups: add the bot as an admin with Manage topics (or @BotFather /setprivacy → Disable, then re-add it), then /telegram group add <chat id> — /status shows the id once the bot is in the group",
    );
    term.say("telegram ready: run t --headless and message the bot");
    Ok(())
}

/// `https://t.me/<username>?start=<code>` (98 §10.4).
#[must_use]
pub fn deep_link(username: &str, code: &str) -> String {
    format!(
        "https://t.me/{}?start={code}",
        username.trim_start_matches('@')
    )
}

/// `Api::new(token).me()`.
async fn verify(token: &str, host: Option<&str>) -> anyhow::Result<(Api, BotIdentity)> {
    let api = Api::new(token, host)?;
    let me = api.me().await?;
    Ok((api, me))
}

/// Prompts for the token up to three times until `getMe` accepts it.
async fn ask_token(
    term: &mut dyn Terminal,
    host: Option<&str>,
) -> anyhow::Result<(String, (Api, BotIdentity))> {
    term.say(
        "1. bot token: @BotFather → /newbot → a name, then a username ending in `bot` → copy the token (123456:ABC-…)",
    );
    for _ in 0..3 {
        let tok = term.secret("bot token: ")?.trim().to_owned();
        match verify(&tok, host).await {
            Ok(v) => return Ok((tok, v)),
            Err(e) => term.say(&format!("   {e:#}")),
        }
    }
    bail!("giving up after 3 attempts")
}

/// One-shot poller (98 §3.1 shape): skips pending updates (`offset: -1`),
/// then long-polls until `/start <code>` arrives or [`PAIR_TTL`] passes.
/// `Ok(None)` on timeout.
async fn wait_for_start(
    api: &Api,
    me: &BotIdentity,
    code: &str,
) -> anyhow::Result<Option<(u64, String)>> {
    let deadline = Instant::now() + PAIR_TTL;
    let mut offset = api
        .get_updates(-1, 1, 0, true)
        .await?
        .last()
        .map_or(0, |u| u.id + 1);
    let mut attempt = 0;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Ok(None);
        }
        let timeout = left.as_secs().min(30);
        let updates = match api.get_updates(offset, 100, timeout, false).await {
            Ok(u) => {
                attempt = 0;
                u
            }
            Err(e) => {
                tracing::warn!(error = %e, "telegram setup: getUpdates");
                tokio::time::sleep(backoff_delay(attempt).min(left)).await;
                attempt += 1;
                continue;
            }
        };
        for u in updates {
            offset = u.id + 1;
            let Incoming::Message(m) = classify(&u.update, me) else {
                continue;
            };
            let Some(from) = m.from else { continue };
            if m.command
                .as_ref()
                .is_some_and(|c| c.name == "start" && !c.for_other_bot && c.arg == code)
            {
                return Ok(Some((from.id, from.name)));
            }
        }
    }
}

/// `Some(file)` holding `<data_dir>/engine.lock`, `None` when an engine
/// holds it (the same advisory lock `rpc::engine::acquire` takes; that crate
/// depends on this one, so the check is inlined).
fn engine_lock(data_dir: &Path) -> anyhow::Result<Option<std::fs::File>> {
    std::fs::create_dir_all(data_dir)?;
    let f = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(data_dir.join("engine.lock"))?;
    match f.try_lock() {
        Ok(()) => Ok(Some(f)),
        Err(std::fs::TryLockError::WouldBlock) => Ok(None),
        Err(std::fs::TryLockError::Error(e)) => Err(e.into()),
    }
}

/// The vault as the daemon would open it: `secrets.key_store` from
/// `config.yaml` (or `THEUTH_SECRETS__KEY_STORE`), default `auto`.
// ponytail: a line sniff instead of Figment — figment/serde_yaml are
// dev-dependencies only here; swap in `Figment` if the section grows.
async fn open_vault(data_dir: &Path, config: &Path) -> anyhow::Result<secrets::Vault> {
    let text = std::fs::read_to_string(config).unwrap_or_default();
    let word = std::env::var("THEUTH_SECRETS__KEY_STORE")
        .ok()
        .or_else(|| config_value(&text, "secrets", "key_store"));
    let key_store = match word {
        Some(w) => serde_json::from_value(serde_json::Value::String(w.clone()))
            .with_context(|| format!("secrets.key_store: {w:?}"))?,
        None => secrets::KeyStore::default(),
    };
    secrets::Vault::open(data_dir, key_store).await
}

/// `telegram.owner` from config.yaml when set.
fn current_owner(config: &Path) -> Option<u64> {
    let text = std::fs::read_to_string(config).ok()?;
    config_value(&text, "telegram", "owner")?.parse().ok()
}

/// `section.key`'s scalar in a flat two-level YAML file; quotes stripped.
fn config_value(text: &str, section: &str, key: &str) -> Option<String> {
    let mut inside = false;
    for line in text.lines() {
        let indented = line.starts_with([' ', '\t']);
        let bare = line.split('#').next().unwrap_or("").trim_end();
        if bare.trim().is_empty() {
            continue;
        }
        if !indented {
            inside = bare == format!("{section}:");
            continue;
        }
        if !inside {
            continue;
        }
        if let Some(v) = bare.trim_start().strip_prefix(&format!("{key}:")) {
            let v = v.trim().trim_matches(['"', '\'']);
            return (!v.is_empty() && v != "null" && v != "~").then(|| v.to_owned());
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Arc;

    use parking_lot::Mutex;
    use serde_json::{Value, json};
    use wiremock::matchers::{body_partial_json, path};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    use super::*;

    /// Scripted answers; output shared so a mock responder can read the
    /// deep link the setup printed.
    #[derive(Default)]
    struct Fake {
        answers: VecDeque<&'static str>,
        out: Arc<Mutex<Vec<String>>>,
    }

    impl Terminal for Fake {
        fn prompt(&mut self, q: &str) -> anyhow::Result<String> {
            self.out.lock().push(q.to_owned());
            Ok(self.answers.pop_front().expect("unexpected prompt").to_owned())
        }
        fn secret(&mut self, q: &str) -> anyhow::Result<String> {
            self.prompt(q)
        }
        fn say(&mut self, s: &str) {
            self.out.lock().push(s.to_owned());
        }
    }

    impl Fake {
        fn text(&self) -> String {
            self.out.lock().join("\n")
        }
    }

    /// The code in the printed deep link, once it is there.
    fn printed_code(out: &Mutex<Vec<String>>) -> Option<String> {
        let text = out.lock().join("\n");
        let i = text.find("?start=")? + 7;
        text[i..].split_whitespace().next().map(str::to_owned)
    }

    const TOKEN: &str = "123:TEST";

    fn ok(result: Value) -> ResponseTemplate {
        ResponseTemplate::new(200).set_body_json(json!({ "ok": true, "result": result }))
    }

    fn me_json(topics: bool) -> Value {
        json!({
            "id": 42, "is_bot": true, "first_name": "T", "username": "theuth_bot",
            "can_join_groups": true, "can_read_all_group_messages": false,
            "supports_inline_queries": false, "has_topics_enabled": topics,
            "allows_users_to_create_topics": false, "can_connect_to_business": false,
            "can_manage_bots": false, "has_main_web_app": false,
            "supports_guest_queries": false, "supports_join_request_queries": false
        })
    }

    fn start_update(id: i64, text: &str) -> Value {
        json!({
            "update_id": id,
            "message": {
                "message_id": id, "date": 1,
                "chat": { "id": 500, "type": "private", "first_name": "Ann" },
                "from": { "id": 500, "is_bot": false, "first_name": "Ann", "last_name": "Lee" },
                "text": text,
                "entities": [{ "type": "bot_command", "offset": 0, "length": 6 }]
            }
        })
    }

    /// getMe and setMyCommands for [`TOKEN`] only (a bad token 404s).
    async fn server(topics: bool) -> MockServer {
        let s = MockServer::start().await;
        Mock::given(path(format!("/bot{TOKEN}/getMe")))
            .respond_with(ok(me_json(topics)))
            .mount(&s)
            .await;
        Mock::given(path(format!("/bot{TOKEN}/setMyCommands")))
            .respond_with(ok(json!(true)))
            .mount(&s)
            .await;
        s
    }

    /// `getUpdates` with `offset` → `result`.
    async fn updates(s: &MockServer, offset: i64, result: Value) {
        Mock::given(path(format!("/bot{TOKEN}/getUpdates")))
            .and(body_partial_json(json!({ "offset": offset })))
            .respond_with(ok(result))
            .mount(s)
            .await;
    }

    /// A data dir whose vault key lives in a file (never the keychain).
    fn data_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("config.yaml"),
            "secrets:\n  key_store: file\n",
        )
        .unwrap();
        dir
    }

    async fn methods(s: &MockServer) -> Vec<(String, Value)> {
        s.received_requests()
            .await
            .unwrap()
            .iter()
            .map(|r| {
                (
                    r.url.path().rsplit('/').next().unwrap().to_owned(),
                    serde_json::from_slice(&r.body).unwrap_or(Value::Null),
                )
            })
            .collect()
    }

    #[tokio::test]
    async fn full_run_writes_token_commands_and_owner() {
        let s = server(false).await;
        // Skip call: one pending update → the long poll starts at 10.
        updates(&s, -1, json!([start_update(9, "/start old")])).await;
        // First long poll: a wrong code is ignored and the offset advances.
        updates(&s, 10, json!([start_update(10, "/start wrong")])).await;
        // Second: the code the setup printed (read from the terminal output).
        let mut term = Fake {
            answers: VecDeque::from(["bad", TOKEN]),
            ..Default::default()
        };
        let out = Arc::clone(&term.out);
        Mock::given(path(format!("/bot{TOKEN}/getUpdates")))
            .and(body_partial_json(json!({ "offset": 11 })))
            .respond_with(move |_: &Request| {
                let code = printed_code(&out).expect("link printed before polling");
                ok(json!([start_update(11, &format!("/start {code}"))]))
            })
            .mount(&s)
            .await;
        let dir = data_dir();
        run_with_host(dir.path(), &mut term, Some(&s.uri()))
            .await
            .unwrap();

        let text = term.text();
        assert!(text.contains("@theuth_bot · topics off"), "{text}");
        assert!(text.contains("Bot Settings → Topics"), "{text}");
        assert!(text.contains("2. token saved"), "{text}");
        assert!(text.contains("paired with Ann Lee (500)"), "{text}");
        assert!(text.contains("https://t.me/theuth_bot?start="), "{text}");
        assert!(text.ends_with("telegram ready: run t --headless and message the bot"));
        let config = std::fs::read_to_string(dir.path().join("config.yaml")).unwrap();
        assert!(config.contains("telegram:"), "{config}");
        assert_eq!(current_owner(&dir.path().join("config.yaml")), Some(500));
        assert!(config.contains("enabled: true"), "{config}");
        let vault = secrets::Vault::open(dir.path(), secrets::KeyStore::File)
            .await
            .unwrap();
        assert_eq!(vault.get(SECRET_TOKEN, 0).as_deref().map(String::as_str), Some(TOKEN));

        // The rejected token's getMe (`/botbad/getMe`, 404) is received too.
        let reqs = methods(&s).await;
        let names: Vec<&str> = reqs.iter().map(|(m, _)| m.as_str()).collect();
        assert_eq!(
            names,
            [
                "getMe",
                "getMe",
                "setMyCommands",
                "setMyCommands",
                "getUpdates",
                "getUpdates",
                "getUpdates",
                "setMyCommands"
            ],
            "{names:?}"
        );
        let scopes: Vec<&Value> = reqs
            .iter()
            .filter(|(m, _)| m == "setMyCommands")
            .map(|(_, b)| &b["scope"])
            .collect();
        assert_eq!(scopes[0]["type"], "default");
        assert_eq!(scopes[1]["type"], "all_group_chats");
        assert_eq!(scopes[2]["type"], "chat");
        assert_eq!(scopes[2]["chat_id"], 500);
        assert_eq!(reqs[4].1["offset"], -1);
        assert_eq!(reqs[4].1["limit"], 1);
        assert_eq!(reqs[5].1["limit"], 100);
        assert_eq!(reqs[5].1["timeout"], 30);
        assert_eq!(reqs[6].1["offset"], 11);
    }

    #[tokio::test]
    async fn valid_token_and_owner_are_kept() {
        let s = server(true).await;
        let dir = data_dir();
        let vault = secrets::Vault::open(dir.path(), secrets::KeyStore::File)
            .await
            .unwrap();
        vault
            .set(SECRET_TOKEN, TOKEN, None, "test")
            .await
            .unwrap();
        drop(vault);
        let config = dir.path().join("config.yaml");
        runtime::write_config_value(&config, "telegram", "owner", "77").unwrap();
        let mut term = Fake {
            answers: VecDeque::from(["n"]),
            ..Default::default()
        };
        run_with_host(dir.path(), &mut term, Some(&s.uri()))
            .await
            .unwrap();
        let text = term.text();
        assert!(text.contains("1. bot token ok (existing)"), "{text}");
        assert!(text.contains("topics on"), "{text}");
        assert!(!text.contains("Bot Settings → Topics"), "{text}");
        assert!(text.contains("2. token already in the vault"), "{text}");
        assert!(text.contains("owner 77 already set; re-pair?"), "{text}");
        assert!(text.contains("owner 77 kept"), "{text}");
        assert!(!text.contains("?start="), "{text}");
        assert_eq!(current_owner(&config), Some(77));
        let names: Vec<String> = methods(&s).await.into_iter().map(|(m, _)| m).collect();
        assert_eq!(
            names,
            ["getMe", "setMyCommands", "setMyCommands", "setMyCommands"]
        );
    }

    #[tokio::test]
    async fn running_engine_short_circuits() {
        let dir = tempfile::tempdir().unwrap();
        let lock = std::fs::File::create(dir.path().join("engine.lock")).unwrap();
        lock.lock().unwrap();
        let mut term = Fake::default();
        run_with_host(dir.path(), &mut term, Some("http://127.0.0.1:9"))
            .await
            .unwrap();
        assert!(term.text().ends_with(DAEMON_RUNNING));
        assert!(!dir.path().join("config.yaml").exists());
    }

    #[test]
    fn config_value_reads_two_level_yaml() {
        let y = "a: 1\nsecrets:\n  tools: true\n  key_store: \"file\" # c\ntelegram:\n  owner: 500\n  enabled: true\n";
        assert_eq!(config_value(y, "secrets", "key_store").as_deref(), Some("file"));
        assert_eq!(config_value(y, "telegram", "owner").as_deref(), Some("500"));
        assert_eq!(config_value(y, "telegram", "key_store"), None);
        assert_eq!(config_value(y, "slack", "owner"), None);
        assert_eq!(config_value("telegram:\n  owner: null\n", "telegram", "owner"), None);
    }

    #[test]
    fn deep_link_strips_the_at_and_its_qr_is_small() {
        assert_eq!(
            deep_link("@theuth_bot", "abc"),
            "https://t.me/theuth_bot?start=abc"
        );
        let url = deep_link("theuth_bot", &mint_code());
        assert!(link::qr(&url).chars().count() < 2000);
    }
}
