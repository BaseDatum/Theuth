//! `t plugin …` (design doc 216 §12; contract §7.2). Owner: B2 — B0 wrote the enums and a stub
//! `run`.

use std::path::Path;

/// `t plugin …` (216 §12).
#[derive(clap::Subcommand, Debug)]
#[allow(dead_code)] // B2: `run` reads every field
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

/// Run one `t plugin` subcommand.
pub(crate) async fn run(data_dir: &Path, config: &figment::Figment, cmd: PluginCmd) -> anyhow::Result<()> {
    let _ = (data_dir, config, cmd);
    anyhow::bail!("t plugin: not implemented yet (216 B0)")
}
