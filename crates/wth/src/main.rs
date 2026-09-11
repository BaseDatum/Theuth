//! `wth`: build the runtime, start the plugin list, wait for shutdown.
//! Also the Python runtime worker: `wth subprocess` (design doc 08 §9, [`worker`]).

mod worker;

use std::path::{Path, PathBuf};

use clap::Parser;
use figment::Figment;
use figment::providers::{Env, Format, Yaml};
use runtime::{Plugin, Runtime, SessionId};

/// The worker's memory limits are enforced by the allocator, so the process
/// that runs `wth subprocess` must own it. Unarmed (every other mode) it is
/// `System` plus one live-byte counter.
#[global_allocator]
static ALLOC: monty_alloc::LimitedAllocator = monty_alloc::LimitedAllocator;

#[derive(Parser, Debug)]
#[command(name = "wth", version, about = "what the harness")]
struct Cli {
    /// Data directory (config, credentials, mcp/, sqlite, logs, token, skills).
    #[arg(long, env = "WTH_DATA_DIR")]
    data_dir: Option<PathBuf>,
    /// Config file; defaults to `<data_dir>/config.yaml`.
    #[arg(long)]
    config: Option<PathBuf>,
    /// Start the JSON-RPC server plugin.
    #[arg(long)]
    serve: bool,
    /// Do not start the terminal UI (implies --serve).
    #[arg(long)]
    headless: bool,
    /// Open an existing session in the terminal UI.
    #[arg(long)]
    resume: Option<SessionId>,
    /// Print mode: send this prompt (or stdin when omitted), stream the reply
    /// to stdout, exit when the turn finishes. No full-screen UI.
    #[arg(short, long, num_args = 0..=1, default_missing_value = "")]
    prompt: Option<String>,
    /// Pair this machine as a Signal linked device (one-shot: prints a QR,
    /// waits for the scan, writes `signal.owner`/`device_name`, exits).
    #[arg(long)]
    link_signal: bool,
    /// Owner E.164 (`+15551234567`) for --link-signal; prompted on a TTY when absent.
    #[arg(long, requires = "link_signal")]
    owner: Option<String>,
    /// Device name shown in the phone's Linked devices list (default: config `signal.device_name`).
    #[arg(long, requires = "link_signal")]
    device_name: Option<String>,
}

const DEFAULT_CONFIG: &str = "# wth configuration. Every plugin reads its own section.
# Env overrides: WTH_<SECTION>__<KEY>, e.g. WTH_RPC__LISTEN=127.0.0.1:7799

agent:
  model: anthropic/claude-sonnet-4-5
  # subagents:
  #   model: null              # default: agent.model
  #   max_depth: 1
  #   max_concurrent: 8
  #   max_runtime_secs: 600
  #   reply_timeout_secs: 120
  # knowledge:                # loop policy (design doc 10 §5); the store itself is `knowledge:` below
  #   hint_limit: 5            # lines per <knowledge> note
  #   hint_rounds: 4           # mid-turn hint cadence in rounds
  #   harvest_every: 3         # tool-using root turns per harvest; 0 disables harvest
  #   harvest_max: 8           # documents accepted per harvest

# knowledge:                 # design doc 10; the bundle lives in <data_dir>/knowledge/
#   enabled: true            # false: no store, no knowledge_* tools, no hints, no harvest

tools:
  policy:
    read_only: allow
    mutating: ask
    destructive: ask
  # approvals: false   # never prompt; `/approvals off always` writes this

rpc:
  listen: 127.0.0.1:7777

# signal:                   # phone frontend; runs in `wth --headless`. Pair with `wth --link-signal`.
#   enabled: true           # daemon starts the channel when linked
#   device_name: wth        # shown in the phone's Linked devices list
#   owner: \"+15551234567\"   # set by --link-signal / signal_link; owner-only commands
#   allowed: []             # more E.164s; each gets its own thread
";

const DEFAULT_CREDENTIALS: &str =
    "# Provider API keys, one per line. The file beats the environment.
# anthropic: sk-ant-...
# openai: sk-...
# gemini: AIza...
# groq: gsk_...
# ollama needs no key.
";

/// Creates the data-dir skeleton on first run. Existing files are never overwritten.
fn first_run(data_dir: &Path, config_path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir.join("mcp"))?;
    if !config_path.exists() {
        std::fs::write(config_path, DEFAULT_CONFIG)?;
    }
    let prompt = data_dir.join("prompt.md");
    if !prompt.exists() {
        std::fs::write(
            prompt,
            "<!-- Standing instructions for the assistant; read at every model call. -->\n",
        )?;
    }
    let creds = data_dir.join("credentials.yaml");
    if !creds.exists() {
        let mut open = std::fs::OpenOptions::new();
        open.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            open.mode(0o600);
        }
        use std::io::Write;
        open.open(&creds)?
            .write_all(DEFAULT_CREDENTIALS.as_bytes())?;
    }
    Ok(())
}

fn main() -> anyhow::Result<()> {
    // Before clap, tokio, logging or anything that could touch stdout: the
    // worker's stdout is the protocol channel (08 §9).
    if std::env::args().nth(1).as_deref() == Some("subprocess") {
        std::process::exit(i32::from(worker::run()));
    }
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run())
}

async fn run() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let data_dir = cli
        .data_dir
        .or_else(|| dirs::home_dir().map(|h| h.join(".wth")))
        .ok_or_else(|| anyhow::anyhow!("cannot determine data dir; pass --data-dir"))?;

    let config_path = cli.config.unwrap_or_else(|| data_dir.join("config.yaml"));
    first_run(&data_dir, &config_path)?;
    let config = Figment::new()
        .merge(Yaml::file(&config_path))
        .merge(Env::prefixed("WTH_").split("__"));

    // The TUI owns the terminal; logs go to a file.
    let logs = data_dir.join("logs");
    std::fs::create_dir_all(&logs)?;
    let (writer, _guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::daily(&logs, "wth.log"));
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?),
        )
        .with_writer(writer)
        .with_ansi(false)
        .init();

    let rt = Runtime::new(config, data_dir);

    // Pairing needs no TUI, agent or tools; the plugin cancels `shutdown`
    // when the phone has scanned (12 §3).
    let plugins: Vec<Box<dyn Plugin>> = if cli.link_signal {
        vec![Box::new(signal::SignalPlugin {
            channel: false,
            link: Some(signal::LinkOpts {
                owner: cli.owner,
                device_name: cli.device_name,
            }),
        })]
    } else {
        let mut plugins: Vec<Box<dyn Plugin>> = vec![
            Box::new(tools::ToolsPlugin),
            Box::new(knowledge::KnowledgePlugin),
            Box::new(agent::AgentPlugin),
            Box::new(signal::SignalPlugin {
                channel: cli.headless,
                link: None,
            }),
        ];
        if cli.serve || cli.headless {
            plugins.push(Box::new(rpc::RpcPlugin));
        }
        if !cli.headless {
            let prompt = match cli.prompt {
                Some(p) if p.is_empty() => Some(std::io::read_to_string(std::io::stdin())?),
                p => p,
            };
            plugins.push(Box::new(tui::TuiPlugin {
                resume: cli.resume,
                prompt,
            }));
        }
        plugins
    };

    for plugin in plugins {
        let name = plugin.name();
        plugin
            .start(&rt)
            .await
            .map_err(|e| anyhow::anyhow!("plugin {name} failed to start: {e:#}"))?;
        tracing::info!(plugin = name, "started");
    }

    tokio::select! {
        _ = rt.shutdown.cancelled() => {}
        _ = tokio::signal::ctrl_c() => rt.shutdown.cancel(),
    }
    // Give actors a moment to flush (sqlite writes, terminal restore), then
    // leave without waiting for the runtime: a `spawn_blocking` pipe drain
    // whose child never closed its end would otherwise pin the process.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    drop(_guard);
    std::process::exit(0)
}
