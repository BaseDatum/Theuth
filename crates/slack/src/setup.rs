//! `wth --slack-setup` (21 §10.3): create or update the Slack app from
//! [`crate::MANIFEST`], collect and verify the bot and app-level tokens and
//! the owner, write the tokens into the secrets vault (37 §3.6: `slack_bot`,
//! `slack_app`) and the owner into `config.yaml`. Every step is re-runnable;
//! a token already in the vault that still verifies is skipped.

use std::path::Path;

use anyhow::{Context, bail};
use figment::Figment;
use figment::providers::{Env, Format, Yaml};
use serde_json::{Value, json};

/// Terminal I/O of the setup, so tests can script it.
pub trait SetupIo {
    /// Prints `text` (no newline) and returns the trimmed line typed.
    fn prompt(&mut self, text: &str) -> anyhow::Result<String>;
    /// Prints one line.
    fn say(&mut self, text: &str);
}

/// [`SetupIo`] over stdin/stdout. Tokens echo; they are pasted, not typed.
pub struct Terminal;

impl SetupIo for Terminal {
    fn prompt(&mut self, text: &str) -> anyhow::Result<String> {
        use std::io::Write;
        print!("{text}");
        std::io::stdout().flush()?;
        let mut s = String::new();
        if std::io::stdin().read_line(&mut s)? == 0 {
            bail!("stdin closed");
        }
        Ok(s.trim().to_owned())
    }

    fn say(&mut self, text: &str) {
        println!("{text}");
    }
}

/// The Slack Web API as the setup uses it, so tests can script responses.
#[async_trait::async_trait]
pub trait SetupApi: Send + Sync {
    /// `POST https://slack.com/api/<method>` with `Authorization: Bearer
    /// <token>` and a JSON body → the parsed response when `ok`, else the
    /// `error` code (plus `errors` for manifest methods).
    async fn call(&self, method: &str, token: &str, body: Value) -> anyhow::Result<Value>;
}

/// [`SetupApi`] over reqwest.
pub struct Live(reqwest::Client);

impl Default for Live {
    fn default() -> Self {
        // Same reason as `api::Api::new`: two rustls providers are linked.
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        }
        Self(reqwest::Client::new())
    }
}

#[async_trait::async_trait]
impl SetupApi for Live {
    async fn call(&self, method: &str, token: &str, body: Value) -> anyhow::Result<Value> {
        let r: Value = self
            .0
            .post(format!("https://slack.com/api/{method}"))
            .bearer_auth(token)
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if r["ok"].as_bool() == Some(true) {
            return Ok(r);
        }
        let code = r["error"].as_str().unwrap_or("unknown error");
        match r.get("errors") {
            Some(errors) => bail!("{method}: {code} {errors}"),
            None => bail!("{method}: {code}"),
        }
    }
}

/// Runs the setup against Slack. `app`: an existing app id (`A…`) to bring
/// to the manifest with `apps.manifest.update` instead of creating one.
pub async fn run(data_dir: &Path, app: Option<String>, io: &mut dyn SetupIo) -> anyhow::Result<()> {
    run_with(data_dir, app, io, &Live::default()).await
}

async fn run_with(
    data_dir: &Path,
    app: Option<String>,
    io: &mut dyn SetupIo,
    api: &dyn SetupApi,
) -> anyhow::Result<()> {
    io.say("wth slack setup — stop any running `wth --headless` first");
    let manifest: Value = serde_yaml::from_str(crate::MANIFEST).context("manifest.yaml")?;
    let manifest = manifest.to_string();

    // 1. App from the manifest.
    io.say(
        "1. app configuration token: https://api.slack.com/apps → \"Your App Configuration Tokens\" → Generate (xoxe.xoxp-…, valid 12 h)",
    );
    let (app_id, install_url, what) = ask(io, "config token: ", async |tok| match &app {
        Some(id) => {
            let r = api
                .call(
                    "apps.manifest.update",
                    &tok,
                    json!({"app_id": id, "manifest": manifest}),
                )
                .await?;
            let what = if r["permissions_updated"].as_bool() == Some(true) {
                "updated (scopes changed: reinstall)"
            } else {
                "updated"
            };
            Ok((
                id.clone(),
                format!("https://api.slack.com/apps/{id}/install-on-team"),
                what,
            ))
        }
        None => {
            let r = api
                .call("apps.manifest.create", &tok, json!({"manifest": manifest}))
                .await?;
            let id = r["app_id"]
                .as_str()
                .context("apps.manifest.create: no app_id")?;
            let url = r["oauth_authorize_url"]
                .as_str()
                .context("apps.manifest.create: no oauth_authorize_url")?;
            Ok((id.to_owned(), url.to_owned(), "created"))
        }
    })
    .await?;
    io.say(&format!("   app {app_id} {what}"));

    // 2. Bot token.
    let config = data_dir.join("config.yaml");
    let vault = open_vault(data_dir, &config)?;
    let bot = match vault.get("slack_bot", 0) {
        Some(t) if api.call("auth.test", &t, json!({})).await.is_ok() => {
            io.say("2. bot token ok (existing)");
            None
        }
        _ => {
            io.say(&format!(
                "2. install the app: {install_url}\n   then copy the Bot User OAuth Token (xoxb-…) from OAuth & Permissions"
            ));
            let (tok, who) = ask(io, "bot token: ", async |tok| {
                let r = api.call("auth.test", &tok, json!({})).await?;
                let who = format!(
                    "@{} in {}",
                    r["user"].as_str().unwrap_or("?"),
                    r["team"].as_str().unwrap_or("?")
                );
                Ok((tok, who))
            })
            .await?;
            io.say(&format!("   bot {who}"));
            Some(tok)
        }
    };

    // 3. App-level token.
    let app_tok = match vault.get("slack_app", 0) {
        Some(t)
            if api
                .call("apps.connections.open", &t, json!({}))
                .await
                .is_ok() =>
        {
            io.say("3. app-level token ok (existing)");
            None
        }
        _ => {
            io.say(&format!(
                "3. app-level token: https://api.slack.com/apps/{app_id}/general → App-Level Tokens → Generate with scope connections:write (xapp-…)"
            ));
            let tok = ask(io, "app-level token: ", async |tok| {
                api.call("apps.connections.open", &tok, json!({})).await?;
                Ok(tok)
            })
            .await?;
            io.say("   socket mode ok");
            Some(tok)
        }
    };

    // 4. Owner.
    let current = current_owner(&config);
    io.say("4. owner: your Slack member id (profile → ⋮ More → Copy member ID)");
    let prompt = match &current {
        Some(c) => format!("owner [{c}]: "),
        None => "owner (U…): ".to_owned(),
    };
    let owner = ask(io, &prompt, async |s| match (owner_id(&s), &current) {
        (Some(id), _) => Ok(id.to_owned()),
        (None, Some(c)) if s.is_empty() => Ok(c.clone()),
        _ => bail!("{s:?} is not a member id (U followed by 8+ capitals/digits)"),
    })
    .await?;

    // 5. Write.
    for (key, value) in [("slack_bot", bot), ("slack_app", app_tok)] {
        if let Some(v) = value {
            vault
                .set(key, &v, Some("Slack token"), "slack setup")
                .map_err(anyhow::Error::msg)?;
        }
    }
    runtime::write_config_value(&config, "slack", "owner", &format!("\"{owner}\""))?;
    runtime::write_config_value(&config, "slack", "enabled", "true")?;
    io.say("slack ready: run wth --headless and DM the bot");
    Ok(())
}

/// Prompts up to three times until `check` accepts the (trimmed) answer.
async fn ask<T>(
    io: &mut dyn SetupIo,
    prompt: &str,
    mut check: impl AsyncFnMut(String) -> anyhow::Result<T>,
) -> anyhow::Result<T> {
    for _ in 0..3 {
        match check(io.prompt(prompt)?.trim().to_owned()).await {
            Ok(v) => return Ok(v),
            Err(e) => io.say(&format!("   {e:#}")),
        }
    }
    bail!("giving up after 3 attempts")
}

/// `^U[A-Z0-9]{8,}$` (21 §10.3: ids only; handles need `users:read`).
fn owner_id(s: &str) -> Option<&str> {
    (s.len() >= 9
        && s.starts_with('U')
        && s[1..]
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()))
    .then_some(s)
}

/// The vault as the daemon would open it: `secrets.key_store` from the same
/// Figment `main` builds (`config.yaml` + `WTH_*` env).
fn open_vault(data_dir: &Path, config: &Path) -> anyhow::Result<secrets::Vault> {
    let figment = Figment::new()
        .merge(Yaml::file(config))
        .merge(Env::prefixed("WTH_").split("__"));
    let cfg: secrets::SecretsConfig = if figment.contains("secrets") {
        figment.extract_inner("secrets")?
    } else {
        secrets::SecretsConfig::default()
    };
    secrets::Vault::open(data_dir, cfg.key_store)
}

/// `slack.owner` from config.yaml when set and non-empty.
fn current_owner(config: &Path) -> Option<String> {
    let text = std::fs::read_to_string(config).ok()?;
    let v: serde_yaml::Value = serde_yaml::from_str(&text).ok()?;
    v.get("slack")?
        .get("owner")?
        .as_str()
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::{Mutex, MutexGuard, PoisonError};

    use super::*;

    fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
        m.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[derive(Default)]
    struct FakeIo {
        answers: VecDeque<&'static str>,
        out: Vec<String>,
    }

    impl SetupIo for FakeIo {
        fn prompt(&mut self, text: &str) -> anyhow::Result<String> {
            self.out.push(text.to_owned());
            Ok(self
                .answers
                .pop_front()
                .expect("unexpected prompt")
                .to_owned())
        }
        fn say(&mut self, text: &str) {
            self.out.push(text.to_owned());
        }
    }

    /// Ordered script: each call pops `(expected method, response)`.
    struct FakeApi {
        script: Mutex<VecDeque<(&'static str, Result<Value, &'static str>)>>,
        calls: Mutex<Vec<(String, String, Value)>>,
    }

    impl FakeApi {
        fn new(script: Vec<(&'static str, Result<Value, &'static str>)>) -> Self {
            Self {
                script: Mutex::new(script.into()),
                calls: Mutex::new(Vec::new()),
            }
        }
        fn methods(&self) -> Vec<String> {
            lock(&self.calls).iter().map(|c| c.0.clone()).collect()
        }
    }

    #[async_trait::async_trait]
    impl SetupApi for FakeApi {
        async fn call(&self, method: &str, token: &str, body: Value) -> anyhow::Result<Value> {
            lock(&self.calls).push((method.to_owned(), token.to_owned(), body));
            let (expected, r) = lock(&self.script).pop_front().expect("unexpected call");
            assert_eq!(method, expected);
            r.map_err(|e| anyhow::anyhow!("{method}: {e}"))
        }
    }

    fn created() -> Value {
        json!({"ok": true, "app_id": "A0NEW", "oauth_authorize_url": "https://slack.com/oauth/v2/authorize?x=1",
               "credentials": {"client_id": "1", "client_secret": "2", "signing_secret": "3"}})
    }
    fn authed() -> Value {
        json!({"ok": true, "user": "wth", "team": "acme", "user_id": "UBOT", "team_id": "T1"})
    }
    fn opened() -> Value {
        json!({"ok": true, "url": "wss://x"})
    }

    fn io(answers: &[&'static str]) -> FakeIo {
        FakeIo {
            answers: answers.iter().copied().collect(),
            out: Vec::new(),
        }
    }

    /// `secrets.key_store: file`: tests run headless, no keychain.
    const KEY_STORE: &str = "secrets:\n  key_store: file\n";

    /// A data dir whose `config.yaml` is [`KEY_STORE`] followed by `rest`.
    fn data_dir(rest: &str) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("config.yaml"), format!("{KEY_STORE}{rest}")).unwrap();
        dir
    }

    fn vault(dir: &tempfile::TempDir) -> secrets::Vault {
        secrets::Vault::open(dir.path(), secrets::KeyStore::File).unwrap()
    }

    #[tokio::test]
    async fn create_path_writes_vault_and_config() {
        let dir = data_dir("agent:\n  model: x\n# slack:\n#   owner: U0\n");
        // An old credentials.yaml is imported at vault open (37 §3.6).
        std::fs::write(dir.path().join("credentials.yaml"), "anthropic: sk-1\n").unwrap();
        let api = FakeApi::new(vec![
            ("apps.manifest.create", Ok(created())),
            ("auth.test", Ok(authed())),
            ("apps.connections.open", Ok(opened())),
        ]);
        let mut io = io(&["xoxe.xoxp-cfg", "xoxb-bot", "xapp-app", "U12345678"]);
        run_with(dir.path(), None, &mut io, &api).await.unwrap();

        assert!(!dir.path().join("credentials.yaml").exists());
        let vault = vault(&dir);
        assert_eq!(vault.get("anthropic", 0).unwrap().as_str(), "sk-1");
        assert_eq!(vault.get("slack_bot", 0).unwrap().as_str(), "xoxb-bot");
        assert_eq!(vault.get("slack_app", 0).unwrap().as_str(), "xapp-app");
        let config = std::fs::read_to_string(dir.path().join("config.yaml")).unwrap();
        assert!(config.contains("agent:\n  model: x\n"), "{config}");
        assert!(config.contains("\nslack:\n"), "{config}");
        assert!(config.contains("\n  owner: \"U12345678\"\n"), "{config}");
        assert!(config.contains("\n  enabled: true\n"), "{config}");

        let calls = lock(&api.calls);
        assert_eq!(calls[0].1, "xoxe.xoxp-cfg");
        let manifest: Value =
            serde_json::from_str(calls[0].2["manifest"].as_str().unwrap()).unwrap();
        assert_eq!(manifest["settings"]["socket_mode_enabled"], json!(true));
        assert_eq!(calls[1].1, "xoxb-bot");
        assert_eq!(calls[2].1, "xapp-app");
        assert!(
            io.out
                .iter()
                .any(|l| l.contains("https://slack.com/oauth/v2/authorize?x=1")),
            "{:?}",
            io.out
        );
        assert!(
            io.out.iter().any(|l| l.contains("@wth in acme")),
            "{:?}",
            io.out
        );
        assert_eq!(
            io.out.last().unwrap(),
            "slack ready: run wth --headless and DM the bot"
        );
    }

    #[tokio::test]
    async fn existing_valid_bot_token_is_skipped() {
        let dir = data_dir("");
        vault(&dir)
            .set("slack_bot", "xoxb-old", None, "test")
            .unwrap();
        let api = FakeApi::new(vec![
            ("apps.manifest.create", Ok(created())),
            ("auth.test", Ok(authed())),
            ("apps.connections.open", Ok(opened())),
        ]);
        let mut io = io(&["cfg", "xapp-new", "U12345678"]);
        run_with(dir.path(), None, &mut io, &api).await.unwrap();
        assert!(
            io.out.contains(&"2. bot token ok (existing)".to_owned()),
            "{:?}",
            io.out
        );
        assert!(!io.out.iter().any(|l| l == "bot token: "));
        assert_eq!(lock(&api.calls)[1].1, "xoxb-old");
        let vault = vault(&dir);
        assert_eq!(vault.get("slack_bot", 0).unwrap().as_str(), "xoxb-old");
        assert_eq!(vault.get("slack_app", 0).unwrap().as_str(), "xapp-new");
    }

    #[tokio::test]
    async fn invalid_bot_token_reprompts_and_invalid_owner_reprompts() {
        let dir = data_dir("");
        let api = FakeApi::new(vec![
            ("apps.manifest.create", Ok(created())),
            ("auth.test", Err("invalid_auth")),
            ("auth.test", Ok(authed())),
            ("apps.connections.open", Ok(opened())),
        ]);
        let mut io = io(&[
            "cfg",
            "xoxb-bad",
            "xoxb-good",
            "xapp",
            "bogus",
            "u12345678",
            "U0ABCDEF12",
        ]);
        run_with(dir.path(), None, &mut io, &api).await.unwrap();
        assert_eq!(io.out.iter().filter(|l| *l == "bot token: ").count(), 2);
        assert_eq!(io.out.iter().filter(|l| *l == "owner (U…): ").count(), 3);
        let config = std::fs::read_to_string(dir.path().join("config.yaml")).unwrap();
        assert!(config.contains("owner: \"U0ABCDEF12\""), "{config}");
        assert_eq!(
            api.methods(),
            [
                "apps.manifest.create",
                "auth.test",
                "auth.test",
                "apps.connections.open"
            ]
        );
    }

    #[tokio::test]
    async fn app_flag_updates_and_enter_keeps_owner() {
        let dir = data_dir("slack:\n  owner: \"UKEEP1234\"\n  enabled: false\n");
        let api = FakeApi::new(vec![
            (
                "apps.manifest.update",
                Ok(json!({"ok": true, "app_id": "A0OLD", "permissions_updated": true})),
            ),
            ("auth.test", Ok(authed())),
            ("apps.connections.open", Ok(opened())),
        ]);
        let mut io = io(&["cfg", "xoxb", "xapp", ""]);
        run_with(dir.path(), Some("A0OLD".into()), &mut io, &api)
            .await
            .unwrap();
        let calls = lock(&api.calls);
        assert_eq!(calls[0].0, "apps.manifest.update");
        assert_eq!(calls[0].2["app_id"], json!("A0OLD"));
        assert!(calls[0].2["manifest"].is_string());
        assert!(
            io.out
                .iter()
                .any(|l| l.contains("https://api.slack.com/apps/A0OLD/install-on-team")),
            "{:?}",
            io.out
        );
        assert!(
            io.out.iter().any(|l| l.contains("scopes changed")),
            "{:?}",
            io.out
        );
        assert!(
            io.out.iter().any(|l| l == "owner [UKEEP1234]: "),
            "{:?}",
            io.out
        );
        let config = std::fs::read_to_string(dir.path().join("config.yaml")).unwrap();
        assert_eq!(
            config,
            format!("{KEY_STORE}slack:\n  owner: \"UKEEP1234\"\n  enabled: true\n")
        );
    }

    #[tokio::test]
    async fn three_failures_give_up() {
        let dir = tempfile::tempdir().unwrap();
        let api = FakeApi::new(vec![
            ("apps.manifest.create", Err("invalid_auth")),
            ("apps.manifest.create", Err("invalid_auth")),
            ("apps.manifest.create", Err("invalid_auth")),
        ]);
        let mut io = io(&["a", "b", "c"]);
        let err = run_with(dir.path(), None, &mut io, &api).await.unwrap_err();
        assert!(err.to_string().contains("3 attempts"), "{err}");
    }

    #[test]
    fn owner_id_shape() {
        assert_eq!(owner_id("U12345678"), Some("U12345678"));
        assert_eq!(owner_id("U0ABCDEF12"), Some("U0ABCDEF12"));
        assert_eq!(owner_id("U1234567"), None);
        assert_eq!(owner_id("W12345678"), None);
        assert_eq!(owner_id("u12345678"), None);
        assert_eq!(owner_id("<@U12345678>"), None);
    }
}
