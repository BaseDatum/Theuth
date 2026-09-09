//! `wth`: build the runtime, start the plugin list, wait for shutdown.

use std::path::PathBuf;

use clap::Parser;
use figment::Figment;
use figment::providers::{Env, Format, Toml};
use runtime::{Plugin, Runtime, SessionId};

#[derive(Parser, Debug)]
#[command(name = "wth", version, about = "what the harness")]
struct Cli {
    /// Data directory (config, sqlite, logs, token, skills).
    #[arg(long, env = "WTH_DATA_DIR")]
    data_dir: Option<PathBuf>,
    /// Config file; defaults to `<data_dir>/config.toml`.
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
}

const DEFAULT_CONFIG: &str = r#"# wth configuration. Every plugin reads its own section.

[agent]
model = "claude-sonnet-4-5"

[tools.policy]
read_only = "allow"
mutating = "ask"
destructive = "ask"

[rpc]
listen = "127.0.0.1:7777"

# [[tools.mcp]]
# name = "everything"
# command = "npx"
# args = ["-y", "@modelcontextprotocol/server-everything"]
"#;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    let data_dir = cli
        .data_dir
        .or_else(|| dirs::data_dir().map(|d| d.join("wth")))
        .ok_or_else(|| anyhow::anyhow!("cannot determine data dir; pass --data-dir"))?;
    std::fs::create_dir_all(&data_dir)?;

    let config_path = cli.config.unwrap_or_else(|| data_dir.join("config.toml"));
    if !config_path.exists() {
        std::fs::write(&config_path, DEFAULT_CONFIG)?;
    }
    let config = Figment::new()
        .merge(Toml::file(&config_path))
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

    let mut plugins: Vec<Box<dyn Plugin>> =
        vec![Box::new(tools::ToolsPlugin), Box::new(agent::AgentPlugin)];
    if cli.serve || cli.headless {
        plugins.push(Box::new(rpc::RpcPlugin));
    }
    if !cli.headless {
        plugins.push(Box::new(tui::TuiPlugin { resume: cli.resume }));
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
    // Give actors a moment to flush (sqlite writes, terminal restore).
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    Ok(())
}
