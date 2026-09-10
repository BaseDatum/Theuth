//! `wth`: build the runtime, start the plugin list, wait for shutdown.

use std::path::{Path, PathBuf};

use clap::Parser;
use figment::Figment;
use figment::providers::{Env, Format, Yaml};
use runtime::{Plugin, Runtime, SessionId, write_config_value};

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

/// `wth --link-signal`: pair, persist owner/device name, exit (design doc 09 §11.1).
async fn link_signal(
    config: &Figment,
    data_dir: &Path,
    config_path: &Path,
    owner: Option<String>,
    device_name: Option<String>,
) -> anyhow::Result<()> {
    let mut cfg: signal::SignalConfig = config.extract_inner("signal").unwrap_or_default();
    if let Some(n) = device_name {
        cfg.device_name = n;
    }
    let owner = owner
        .map(|o| signal::e164(&o))
        .transpose()
        .map_err(anyhow::Error::msg)?;
    let number = signal::link_oneshot(data_dir, &cfg, |url| {
        println!(
            "scan from Signal → Settings → Linked devices (expires in ~90 s; a fresh code follows):\n\n{}\n{url}\n",
            signal::qr(url)
        );
    })
    .await?;
    let owner = match owner {
        Some(o) => o,
        None if !cfg.owner.is_empty() => cfg.owner.clone(),
        None => {
            use std::io::{IsTerminal, Write};
            let mut line = String::new();
            if std::io::stdin().is_terminal() {
                print!("owner number (E.164, e.g. +15551234567): ");
                std::io::stdout().flush()?;
                std::io::stdin().read_line(&mut line)?;
            }
            match signal::e164(line.trim()) {
                Ok(o) => o,
                Err(e) => {
                    eprintln!(
                        "{e}; set signal.owner in {} yourself",
                        config_path.display()
                    );
                    String::new()
                }
            }
        }
    };
    if !owner.is_empty() {
        write_config_value(config_path, "signal", "owner", &format!("{owner:?}"))?;
    }
    write_config_value(config_path, "signal", "device_name", &cfg.device_name)?;
    println!("linked as {number}; owner {owner}; run wth --headless");
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
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

    if cli.link_signal {
        link_signal(&config, &data_dir, &config_path, cli.owner, cli.device_name).await?;
        drop(_guard);
        std::process::exit(0);
    }

    let rt = Runtime::new(config, data_dir);

    let mut plugins: Vec<Box<dyn Plugin>> = vec![
        Box::new(tools::ToolsPlugin),
        Box::new(agent::AgentPlugin),
        Box::new(signal::SignalPlugin {
            channel: cli.headless,
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
