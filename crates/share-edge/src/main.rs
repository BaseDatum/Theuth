//! `theuth-share-edge`: the standalone blind SNI edge of design doc 210 (CLI of §8.5).

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use share_edge::config::{Config, DEFAULT_CONFIG};
use share_proto::Admission;
use share_proto::admin::{AdminCommand, AdminRequest, BanKind, ListWhat};

/// Public web sharing edge: routes visitors by SNI to hosting engines over iroh (`theuth/share/1`), never
/// decrypting; issues name certificates over ACME.
#[derive(Parser)]
#[command(name = "theuth-share-edge", version, about)]
struct Cli {
    /// Config file (YAML); env `THEUTH_SHARE_EDGE_*` overrides it.
    #[arg(long, global = true, default_value = DEFAULT_CONFIG)]
    config: PathBuf,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run the edge.
    Run,
    /// Operator commands over the admin socket.
    Admin {
        /// Print the raw JSON answer instead of YAML.
        #[arg(long, global = true)]
        json: bool,
        /// Operator reference logged with a mutating command (ticket, report id).
        #[arg(long = "ref", global = true)]
        reference: Option<String>,
        #[command(subcommand)]
        cmd: AdminCmd,
    },
    /// ACME account management.
    Acme {
        #[command(subcommand)]
        cmd: AcmeCmd,
    },
    /// Print the edge ticket for `share.edge`.
    Ticket,
}

#[derive(Subcommand)]
enum AdminCmd {
    /// Counts and modes.
    Status,
    /// `names` (default), `meshes` or `tunnels`.
    List {
        #[arg(value_parser = ["names", "meshes", "tunnels"])]
        what: Option<String>,
    },
    /// Ban a name, mesh, engine key or network; closes its tunnels.
    Ban {
        #[arg(value_parser = ["name", "mesh", "key", "net"])]
        kind: String,
        value: String,
        #[arg(long)]
        reason: String,
    },
    /// Lift a ban.
    Unban {
        #[arg(value_parser = ["name", "mesh", "key", "net"])]
        kind: String,
        value: String,
    },
    /// Set admission live: `open`, `invite` or `closed`.
    Admission {
        #[arg(value_parser = ["open", "invite", "closed"])]
        mode: String,
    },
    /// Close every tunnel and refuse `hello` until `resume`.
    Pause,
    /// Undo `pause`.
    Resume,
}

#[derive(Subcommand)]
enum AcmeCmd {
    /// Create the ACME account and print the CAA lines.
    Register {
        #[arg(long, default_value = "letsencrypt", value_parser = ["letsencrypt", "gts"])]
        ca: String,
    },
}

fn ban_kind(s: &str) -> BanKind {
    match s {
        "mesh" => BanKind::Mesh,
        "key" => BanKind::Key,
        "net" => BanKind::Net,
        _ => BanKind::Name,
    }
}

fn admin_command(cmd: AdminCmd) -> AdminCommand {
    match cmd {
        AdminCmd::Status => AdminCommand::Status,
        AdminCmd::List { what } => AdminCommand::List {
            what: what.map(|w| match w.as_str() {
                "meshes" => ListWhat::Meshes,
                "tunnels" => ListWhat::Tunnels,
                _ => ListWhat::Names,
            }),
        },
        AdminCmd::Ban {
            kind,
            value,
            reason,
        } => AdminCommand::Ban {
            kind: ban_kind(&kind),
            value,
            reason,
        },
        AdminCmd::Unban { kind, value } => AdminCommand::Unban {
            kind: ban_kind(&kind),
            value,
        },
        AdminCmd::Admission { mode } => AdminCommand::Admission {
            mode: match mode.as_str() {
                "invite" => Admission::Invite,
                "closed" => Admission::Closed,
                _ => Admission::Open,
            },
        },
        AdminCmd::Pause => AdminCommand::Pause,
        AdminCmd::Resume => AdminCommand::Resume,
    }
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    platform::version::set(env!("CARGO_PKG_VERSION"));
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async move {
        let (cfg, secrets) = Config::load(&cli.config)?;
        match cli.cmd {
            Cmd::Run => share_edge::run(cfg, secrets).await,
            Cmd::Admin {
                json,
                reference,
                cmd,
            } => {
                let req = AdminRequest {
                    id: 1,
                    command: admin_command(cmd),
                    reference,
                };
                let resp = share_edge::admin::request(&cfg.admin_socket(), &req).await?;
                if json {
                    println!("{}", serde_json::to_string(&resp)?);
                } else if let Some(e) = &resp.error {
                    anyhow::bail!("{}: {}", e.code, e.message);
                } else {
                    print!("{}", serde_yaml::to_string(&resp.result)?);
                }
                if !resp.ok {
                    std::process::exit(1);
                }
                Ok(())
            }
            // TODO(EDGE2): instant-acme account creation, `acme/<ca>.json`, the CAA lines of §8.2.
            Cmd::Acme {
                cmd: AcmeCmd::Register { ca },
            } => anyhow::bail!("acme register --ca {ca}: not available in this build yet"),
            Cmd::Ticket => {
                let key = share_edge::load_or_create_key(&cfg.key_path())?;
                println!("{}", share_edge::ticket(&cfg, key.public())?);
                Ok(())
            }
        }
    })
}
