//! `t plugin …` (design doc 216 §12; contract §7.2). Owner: B2.
//!
//! Engine-required subcommands are one RPC call each on the running engine (the `cmd_67`
//! pattern). `new` and `check` fall back to in-process calls when no engine answers; `logs`
//! reads today's log file directly. `approve` is a human press (contract §8): it demands a TTY
//! on stdin and stdout before it connects, shows the card and approves only on a typed `y`.

use std::io::{BufRead as _, IsTerminal as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use jsonrpsee::core::ClientError;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::rpc_params;
use jsonrpsee::ws_client::WsClient;
use plugins::{
    CatalogEntry, CheckReport, InstallReport, Lang, PluginDetail, PluginInfo, PluginsConfig,
    ScaffoldReport,
};

/// `t plugin …` (216 §12).
#[derive(clap::Subcommand, Debug)]
pub(crate) enum PluginCmd {
    /// List plugins with their state.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Scaffold plugins/<name>/ from the SDK template (no engine needed).
    New {
        name: String,
        #[arg(long, value_enum, default_value_t = LangArg::Py)]
        lang: LangArg,
        /// No SDK: the raw protocol template (py or sh).
        #[arg(long)]
        raw: bool,
    },
    /// Copy a local plugin folder into plugins/ and file its install card.
    Install { path: std::path::PathBuf },
    /// Review a plugin's pending card on this terminal and approve it (a human press).
    Approve { name: String },
    /// Enable a plugin (starts it if approved).
    Enable { name: String },
    /// Disable (stop) a plugin.
    Disable { name: String },
    /// Restart a plugin.
    Restart { name: String },
    /// Print today's log (no engine needed).
    Logs {
        name: String,
        #[arg(short = 'n', long, default_value_t = 100)]
        tail: u32,
        #[arg(short, long)]
        follow: bool,
    },
    /// Uninstall: stop, drop the grant, move the folder to plugins/.trash.
    Uninstall {
        name: String,
        #[arg(long)]
        keep_data: bool,
    },
    /// Validate a plugin folder without starting it.
    Check {
        path: std::path::PathBuf,
        /// Also run the plugin with --theuth-manifest and warn where code and plugin.yaml drift.
        #[arg(long)]
        sync: bool,
        #[arg(long)]
        json: bool,
    },
    /// The event catalog plugins can subscribe to.
    Catalog {
        topic: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

/// `--lang` values (Lang).
#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LangArg {
    Py,
    Js,
    Sh,
    Go,
    Rb,
    Rs,
}

impl From<LangArg> for Lang {
    fn from(l: LangArg) -> Self {
        match l {
            LangArg::Py => Lang::Py,
            LangArg::Js => Lang::Js,
            LangArg::Sh => Lang::Sh,
            LangArg::Go => Lang::Go,
            LangArg::Rb => Lang::Rb,
            LangArg::Rs => Lang::Rs,
        }
    }
}

/// Run one `t plugin` subcommand.
pub(crate) async fn run(data_dir: &Path, config: &figment::Figment, cmd: PluginCmd) -> anyhow::Result<()> {
    let cfg: PluginsConfig = config.extract_inner("plugins").unwrap_or_default();
    let plugins_dir = data_dir.join(&cfg.dir);
    match cmd {
        PluginCmd::Logs { name, tail, follow } => logs(data_dir, &name, tail, follow).await,
        PluginCmd::Approve { name } => {
            // The TTY rule runs before anything connects: a script can never approve.
            if !(std::io::stdin().is_terminal() && std::io::stdout().is_terminal()) {
                eprintln!("t plugin approve needs a terminal");
                std::process::exit(2);
            }
            approve(&engine(data_dir, config).await, &name).await
        }
        PluginCmd::New { name, lang, raw } => {
            let report: ScaffoldReport = match try_engine(data_dir, config).await {
                Some(c) => call(c.request("plugin_new", rpc_params![name, Lang::from(lang), raw]).await)?,
                None => match plugins::scaffold::new_plugin(&plugins_dir, &name, lang.into(), raw) {
                    Ok(r) => r,
                    Err(e) => fail(&e.to_string()),
                },
            };
            println!("{}", report.dir);
            for f in &report.files {
                println!("  {f}");
            }
            println!("{}", report.next);
            Ok(())
        }
        PluginCmd::Check { path, sync, json } => {
            let path = absolute(&path)?;
            let report: CheckReport = match try_engine(data_dir, config).await {
                Some(c) => call(c.request("plugin_check", rpc_params![path.to_string_lossy()]).await)?,
                None => plugins::manifest::check_offline(&path),
            };
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_check(&report);
            }
            if sync {
                for w in sync_drift(&path).await {
                    println!("warning: {w}");
                }
            }
            if !report.ok {
                std::process::exit(1);
            }
            Ok(())
        }
        cmd => online(&engine(data_dir, config).await, cmd).await,
    }
}

/// The running engine, or the `cmd_67` hint and exit 1.
async fn engine(data_dir: &Path, config: &figment::Figment) -> WsClient {
    let listen = crate::listen(data_dir, config);
    match crate::link_cli::connect(data_dir, &listen).await {
        Ok(c) => c,
        Err(_) => {
            eprintln!("no harness on {listen}; start `t --headless` first");
            std::process::exit(1);
        }
    }
}

/// The running engine, if one answers.
async fn try_engine(data_dir: &Path, config: &figment::Figment) -> Option<WsClient> {
    let listen = crate::listen(data_dir, config);
    crate::link_cli::connect(data_dir, &listen).await.ok()
}

/// The server's message on stderr, exit 1.
fn fail(msg: &str) -> ! {
    eprintln!("{msg}");
    std::process::exit(1)
}

/// An RPC reply: a call error is printed and exits 1; a transport error propagates.
fn call<T>(r: Result<T, ClientError>) -> anyhow::Result<T> {
    match r {
        Ok(v) => Ok(v),
        Err(ClientError::Call(e)) => fail(e.message()),
        Err(e) => Err(e.into()),
    }
}

fn absolute(path: &Path) -> anyhow::Result<PathBuf> {
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    })
}

/// Serde name of a unit enum value.
fn tag<T: serde::Serialize>(v: &T) -> String {
    match serde_json::to_value(v) {
        Ok(serde_json::Value::String(s)) => s,
        _ => String::new(),
    }
}

/// `NAME  VERSION  STATE[  needs approval][  ERROR]`.
fn info_line(i: &PluginInfo) -> String {
    let mut s = format!("{}  {}  {}", i.name, i.version, tag(&i.state));
    if i.needs_approval {
        s.push_str("  needs approval");
    }
    if let Some(e) = &i.error {
        s.push_str("  ");
        s.push_str(e);
    }
    s
}

/// The engine-required subcommands other than `approve`.
async fn online(c: &WsClient, cmd: PluginCmd) -> anyhow::Result<()> {
    match cmd {
        PluginCmd::List { json } => {
            let list: Vec<PluginInfo> = call(c.request("plugin_list", rpc_params![]).await)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else if list.is_empty() {
                println!("no plugins");
            } else {
                for i in &list {
                    println!("{}", info_line(i));
                }
            }
        }
        PluginCmd::Install { path } => {
            let path = absolute(&path)?;
            let r: InstallReport = call(c.request("plugin_install", rpc_params![path.to_string_lossy()]).await)?;
            println!("installed {} at {}", r.name, r.dir);
            println!("{}", r.next);
        }
        PluginCmd::Enable { name } => state(call(c.request("plugin_set_enabled", rpc_params![name, true]).await)?),
        PluginCmd::Disable { name } => state(call(c.request("plugin_set_enabled", rpc_params![name, false]).await)?),
        PluginCmd::Restart { name } => state(call(c.request("plugin_restart", rpc_params![name]).await)?),
        PluginCmd::Uninstall { name, keep_data } => {
            let () = call(c.request("plugin_uninstall", rpc_params![name.clone(), keep_data]).await)?;
            println!("uninstalled {name}");
        }
        PluginCmd::Catalog { topic, json } => {
            let list: Vec<CatalogEntry> = call(c.request("plugin_catalog", rpc_params![topic]).await)?;
            if json {
                println!("{}", serde_json::to_string_pretty(&list)?);
            } else {
                for e in &list {
                    println!("{}  {}  {}", e.name, tag(&e.scoped), e.describe);
                }
            }
        }
        PluginCmd::Logs { .. } | PluginCmd::Approve { .. } | PluginCmd::New { .. } | PluginCmd::Check { .. } => {
            unreachable!("served by run")
        }
    }
    Ok(())
}

fn state(i: PluginInfo) {
    println!("{}", info_line(&i));
}

/// `t plugin approve <name>`: show the card, ask on the terminal, approve exactly its hashes.
async fn approve(c: &WsClient, name: &str) -> anyhow::Result<()> {
    let d: PluginDetail = call(c.request("plugin_get", rpc_params![name]).await)?;
    let Some(card) = d.card else {
        fail(&format!("nothing to approve for {name} ({})", tag(&d.info.state)));
    };
    let title = match card.kind {
        plugins::CardKind::Install => format!("Install plugin {} {}?", card.plugin, card.version),
        plugins::CardKind::Widen => format!("Plugin {} asks for more", card.plugin),
        plugins::CardKind::CodeChanged => format!("Plugin {} changed its code", card.plugin),
    };
    println!("{title}");
    println!("kind: {}  risk: {}", tag(&card.kind), tag(&card.risk));
    if card.unverified {
        println!("the saved approval for {} could not be verified", card.plugin);
    }
    for l in &card.lines {
        println!("  [{}] {}{}", tag(&l.risk), l.text, if l.new { "  (new)" } else { "" });
    }
    if !card.changed_paths.is_empty() {
        println!("changed:");
        for p in &card.changed_paths {
            println!("  {p}");
        }
    }
    println!("folder: {}", card.dir);
    print!("Approve {} {}? [y/N] ", card.plugin, card.version);
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
        println!("not approved");
        return Ok(());
    }
    let _: PluginInfo = call(
        c.request(
            "plugin_approve",
            rpc_params![name, card.manifest_sha256, card.code_sha256],
        )
        .await,
    )?;
    println!("approved {name}; starting");
    Ok(())
}

fn print_check(r: &CheckReport) {
    for e in &r.errors {
        println!("error: {}: {}", if e.path.is_empty() { "/" } else { &e.path }, e.message);
    }
    for w in &r.warnings {
        println!("warning: {}: {}", if w.path.is_empty() { "/" } else { &w.path }, w.message);
    }
    if !r.lines.is_empty() {
        println!("the install card will say:");
        for l in &r.lines {
            println!("  [{}] {}", tag(&l.risk), l.text);
        }
    }
    println!("{}", if r.ok { "ok" } else { "not ok" });
}

/// Today's log file (local date, as the host names it: `<name>.log.<YYYY-MM-DD>`).
fn log_path(data_dir: &Path, name: &str, day: chrono::NaiveDate) -> PathBuf {
    data_dir.join("logs").join("plugins").join(format!("{name}.log.{}", day.format("%Y-%m-%d")))
}

/// `t plugin logs`: the last `tail` lines; `-f` polls every 500 ms and follows the midnight rollover.
async fn logs(data_dir: &Path, name: &str, tail: u32, follow: bool) -> anyhow::Result<()> {
    // A plugin name is a folder name: nothing that could leave logs/plugins/.
    if name.is_empty() || name.starts_with('.') || name.contains(['/', '\\']) {
        fail(&format!("no plugin named {name}"));
    }
    let mut day = chrono::Local::now().date_naive();
    let mut path = log_path(data_dir, name, day);
    let bytes = std::fs::read(&path).unwrap_or_default();
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    for l in &lines[lines.len().saturating_sub(tail as usize)..] {
        println!("{l}");
    }
    if !follow {
        if bytes.is_empty() && !path.exists() {
            eprintln!("no log today: {}", path.display());
        }
        return Ok(());
    }
    let mut offset = bytes.len() as u64;
    loop {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let today = chrono::Local::now().date_naive();
        if today != day {
            // Drain what the old file got before midnight, then start on the new one.
            print_from(&path, offset)?;
            day = today;
            path = log_path(data_dir, name, day);
            offset = 0;
        }
        offset = print_from(&path, offset)?;
    }
}

/// Print `path` from `offset`; the new offset (0 when the file shrank or vanished).
fn print_from(path: &Path, offset: u64) -> anyhow::Result<u64> {
    use std::io::{Read as _, Seek as _};
    let Ok(mut f) = std::fs::File::open(path) else { return Ok(0) };
    let len = f.metadata()?.len();
    let start = if len < offset { 0 } else { offset };
    f.seek(std::io::SeekFrom::Start(start))?;
    let mut buf = Vec::new();
    f.read_to_end(&mut buf)?;
    if !buf.is_empty() {
        let mut out = std::io::stdout().lock();
        out.write_all(&buf)?;
        out.flush()?;
    }
    Ok(start + buf.len() as u64)
}

/// `t plugin check --sync`: run the plugin with `--theuth-manifest` and list what the code
/// registers that plugin.yaml does not declare. Never an error: problems are warnings.
async fn sync_drift(dir: &Path) -> Vec<String> {
    let manifest = match read_yaml(&dir.join("plugin.yaml")) {
        Ok(v) => v,
        Err(e) => return vec![format!("--sync: {e}")],
    };
    let mut argv = match &manifest["run"] {
        serde_json::Value::Array(a) => a.iter().filter_map(|v| v.as_str().map(str::to_owned)).collect(),
        serde_json::Value::String(s) => shell_words(s),
        _ => Vec::new(),
    };
    if argv.is_empty() {
        return vec!["--sync: plugin.yaml has no run".into()];
    }
    argv.push("--theuth-manifest".into());
    let cwd = manifest["cwd"].as_str().map_or_else(|| dir.to_path_buf(), |c| dir.join(c));
    let child = tokio::process::Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(&cwd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    let out = match tokio::time::timeout(Duration::from_secs(5), child).await {
        Err(_) => return vec![format!("--sync: {} did not answer --theuth-manifest within 5 s", argv.join(" "))],
        Ok(Err(e)) => return vec![format!("--sync: {}: {e}", argv[0])],
        Ok(Ok(o)) => o,
    };
    if !out.status.success() {
        return vec![format!("--sync: {} --theuth-manifest exited {}", argv[0], out.status)];
    }
    match parse_yaml(&String::from_utf8_lossy(&out.stdout)) {
        Ok(code) => drift(&manifest, &code),
        Err(e) => vec![format!("--sync: the plugin's --theuth-manifest output: {e}")],
    }
}

fn read_yaml(path: &Path) -> anyhow::Result<serde_json::Value> {
    parse_yaml(&std::fs::read_to_string(path)?)
}

/// YAML through figment's provider (the binary's YAML reader).
fn parse_yaml(text: &str) -> anyhow::Result<serde_json::Value> {
    use figment::providers::Format as _;
    Ok(figment::Figment::from(figment::providers::Yaml::string(text)).extract()?)
}

/// Names the code registers (`tools`, `commands`, `on`) that plugin.yaml does not declare.
fn drift(manifest: &serde_json::Value, code: &serde_json::Value) -> Vec<String> {
    let names = |v: &serde_json::Value| -> Vec<String> {
        v.as_array()
            .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
            .unwrap_or_default()
    };
    let declared = |list: &str, key: &str, kind: Option<&str>| -> Vec<String> {
        manifest[list]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter(|p| kind.is_none_or(|k| p["kind"].as_str() == Some(k)))
                    .filter_map(|p| p[key].as_str().map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default()
    };
    let mut out = Vec::new();
    let tools = declared("tools", "name", None);
    for t in names(&code["tools"]) {
        if !tools.contains(&t) {
            out.push(format!("the code registers tool {t:?}, plugin.yaml does not declare it under tools:"));
        }
    }
    let commands = declared("placements", "name", Some("command"));
    let plugin = manifest["name"].as_str().unwrap_or_default();
    for c in names(&code["commands"]) {
        if !commands.contains(&c) && c != plugin {
            out.push(format!("the code handles command {c:?}, plugin.yaml has no command placement named so"));
        }
    }
    let subs = manifest["subscribe"].as_array().cloned().unwrap_or_default();
    for on in names(&code["on"]) {
        if !subscribed(&subs, &on) {
            out.push(format!("the code handles event {on:?}, plugin.yaml does not subscribe to it"));
        }
    }
    out
}

/// Whether a `subscribe:` entry covers `on` (`topic`, `topic#Type`, `<plugin>.<event>`).
fn subscribed(subs: &[serde_json::Value], on: &str) -> bool {
    let (topic, ty) = on.split_once('#').map_or((on, None), |(t, y)| (t, Some(y)));
    subs.iter().any(|s| {
        if let Some(t) = s["topic"].as_str() {
            if t != topic {
                return false;
            }
            let types: Vec<&str> = s["types"].as_array().map(|a| a.iter().filter_map(|x| x.as_str()).collect()).unwrap_or_default();
            return ty.is_none_or(|ty| types.is_empty() || types.contains(&ty));
        }
        match s["event"].as_str() {
            Some(e) if e == on => true,
            Some(e) => e.strip_suffix('*').is_some_and(|prefix| on.starts_with(prefix)),
            None => false,
        }
    })
}

/// `run:` as a string: shell words (quotes and backslashes), no shell is run.
fn shell_words(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut started = false;
    let mut quote: Option<char> = None;
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        match (quote, c) {
            (Some(q), c) if c == q => quote = None,
            (Some('"'), '\\') | (None, '\\') => {
                if let Some(n) = chars.next() {
                    cur.push(n);
                }
                started = true;
            }
            (Some(_), c) => cur.push(c),
            (None, '"' | '\'') => {
                quote = Some(c);
                started = true;
            }
            (None, c) if c.is_whitespace() => {
                if started {
                    out.push(std::mem::take(&mut cur));
                    started = false;
                }
            }
            (None, c) => {
                cur.push(c);
                started = true;
            }
        }
    }
    if started {
        out.push(cur);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser as _;

    #[derive(clap::Parser, Debug)]
    struct Cli {
        #[command(subcommand)]
        cmd: PluginCmd,
    }

    fn parse(args: &[&str]) -> PluginCmd {
        Cli::try_parse_from(std::iter::once("plugin").chain(args.iter().copied())).unwrap().cmd
    }

    #[test]
    fn arg_parsing() {
        assert!(matches!(parse(&["new", "demo"]), PluginCmd::New { lang: LangArg::Py, raw: false, .. }));
        assert!(matches!(parse(&["new", "demo", "--lang", "rs"]), PluginCmd::New { lang: LangArg::Rs, .. }));
        assert!(matches!(parse(&["new", "demo", "--lang", "sh", "--raw"]), PluginCmd::New { lang: LangArg::Sh, raw: true, .. }));
        assert!(matches!(parse(&["logs", "demo"]), PluginCmd::Logs { tail: 100, follow: false, .. }));
        assert!(matches!(parse(&["logs", "demo", "-n", "5", "-f"]), PluginCmd::Logs { tail: 5, follow: true, .. }));
        assert!(matches!(parse(&["uninstall", "demo", "--keep-data"]), PluginCmd::Uninstall { keep_data: true, .. }));
        assert!(matches!(parse(&["check", ".", "--sync", "--json"]), PluginCmd::Check { sync: true, json: true, .. }));
        assert!(matches!(parse(&["catalog"]), PluginCmd::Catalog { topic: None, json: false }));
        // There is no way to approve without the prompt.
        assert!(Cli::try_parse_from(["plugin", "approve", "demo", "--yes"]).is_err());
        assert!(Cli::try_parse_from(["plugin", "new", "demo", "--lang", "java"]).is_err());
    }

    #[test]
    fn shell_words_split() {
        assert_eq!(shell_words("python3 main.py"), ["python3", "main.py"]);
        assert_eq!(shell_words(r#"bash -c 'echo "a b"' x\ y """#), ["bash", "-c", "echo \"a b\"", "x y", ""]);
    }

    #[test]
    fn drift_lists_undeclared_names() {
        let manifest = serde_json::json!({
            "name": "turns",
            "tools": [{"name": "count"}],
            "placements": [{"id": "reset", "kind": "command", "name": "reset"}],
            "subscribe": [{"topic": "agent.events", "types": ["TurnFinished"]}, {"event": "clock.*"}],
        });
        let code = serde_json::json!({
            "tools": ["count", "extra"],
            "commands": ["turns", "reset", "other"],
            "on": ["agent.events#TurnFinished", "agent.events#TextDelta", "clock.tick"],
        });
        let w = drift(&manifest, &code);
        assert_eq!(w.len(), 3, "{w:?}");
        assert!(w[0].contains("\"extra\"") && w[1].contains("\"other\"") && w[2].contains("TextDelta"));
    }

    #[test]
    fn sdk_manifest_output_parses() {
        let v = parse_yaml("# registered in code\ntools: [\"presses\"]\nactions: [inc]\non: []\n").unwrap();
        assert_eq!(v["tools"], serde_json::json!(["presses"]));
        assert_eq!(v["on"], serde_json::json!([]));
    }
}
