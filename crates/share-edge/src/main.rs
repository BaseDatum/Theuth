//! `theuth-share-edge`: the standalone blind SNI edge of design doc 210 (CLI of §8.5).

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use share_edge::config::{Config, DEFAULT_CONFIG};
use share_proto::admin::{AdminCommand, AdminRequest, BanKind, InviteMinted, ListWhat, RevokeReason};
use share_proto::{Admission, PublicKey};

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
    /// Print the edge ticket for `share.edge`; `--standby` prints the standby key's ticket (§5.5).
    Ticket {
        #[arg(long)]
        standby: bool,
    },
    /// With the edge stopped: sign a retire notice of the live key with the standby key, write `retire.json`
    /// and promote the standby key to the live key (§5.5). Launch procedure (§13, `standby_public` set): pass the
    /// offline standby secret with `--standby-key`; `--edge-key` names the live key file (default: the config's
    /// `<data_dir>/iroh.key`), and the files are written beside it. Without `--standby-key`: the on-volume
    /// `iroh.standby.key` (only when `standby_public` is unset).
    Retire {
        /// The offline standby secret key file (`keygen --out`).
        #[arg(long)]
        standby_key: Option<PathBuf>,
        /// The live key file to retire; `retire.json`, the promoted key and `<file>.retired` go beside it.
        /// Without it the config is read for `<data_dir>/iroh.key`.
        #[arg(long, requires = "standby_key")]
        edge_key: Option<PathBuf>,
    },
    /// Offline, no config: write a new endpoint secret key (0600, never overwrites) and print its public key
    /// (hex, as `standby_public` and share-proto parse it; then z-base-32). §13: the standby key.
    Keygen {
        /// Where to write the secret key.
        #[arg(long)]
        out: PathBuf,
    },
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
    /// The certificate slot queue.
    Queue,
    /// One mesh: names, engines, revoke list, rekey history.
    Mesh { id: PublicKey },
    /// Release a name (cooldown for anyone but its holder; certificate revoked).
    Release { name: String },
    /// Evict names never live `evict_unpublished_after` after allocation.
    EvictUnpublished,
    /// Move a name, or every name of a mesh, to another mesh (operator judgement, §5.7; pass `--ref`).
    Recover {
        target: String,
        #[arg(long = "to-mesh")]
        to_mesh: PublicKey,
    },
    /// Invites: `mint`, `list`, `revoke`.
    Invite {
        #[command(subcommand)]
        cmd: InviteCmd,
    },
    /// Resolve and check the CAA set now; a failure closes admission under `require_caa`.
    CaaCheck,
    /// Revoke a name's certificate.
    RevokeCert {
        name: String,
        #[arg(value_parser = ["unspecified", "key_compromise", "superseded", "cessation_of_operation"])]
        reason: Option<String>,
    },
    /// Certificate jobs: `retry <name>`.
    Cert {
        #[command(subcommand)]
        cmd: CertCmd,
    },
    /// Re-read the reloadable config (limits, brand file, terms, admission, brake).
    Reload,
    /// Which brake conditions hold.
    BrakeStatus,
}

#[derive(Subcommand)]
enum InviteCmd {
    /// Mint an invite and print the ticket carrying it.
    #[command(alias = "new")]
    Mint {
        #[arg(long)]
        uses: Option<u32>,
        /// Lifetime, e.g. `7d`.
        #[arg(long, value_parser = humantime::parse_duration)]
        expires: Option<std::time::Duration>,
        #[arg(long)]
        note: Option<String>,
    },
    /// Live invites (hashes only).
    List,
    /// Revoke an invite by its hash (or a unique prefix of 8+ hex characters).
    Revoke { hash: String },
}

#[derive(Subcommand)]
enum CertCmd {
    /// Make the name's queued job due now and reset its backoff (after an infrastructure fix).
    Retry { name: String },
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
        AdminCmd::Queue => AdminCommand::Queue,
        AdminCmd::Mesh { id } => AdminCommand::Mesh { id },
        AdminCmd::Release { name } => AdminCommand::Release { name },
        AdminCmd::EvictUnpublished => AdminCommand::EvictUnpublished,
        AdminCmd::Recover { target, to_mesh } => AdminCommand::Recover { target, to_mesh },
        AdminCmd::Invite { cmd } => match cmd {
            InviteCmd::Mint { uses, expires, note } => AdminCommand::InviteNew {
                uses,
                expires_s: expires.map(|d| d.as_secs()),
                note,
            },
            InviteCmd::List => AdminCommand::InviteList,
            InviteCmd::Revoke { hash } => AdminCommand::InviteRevoke { hash },
        },
        AdminCmd::CaaCheck => AdminCommand::CaaCheck,
        AdminCmd::RevokeCert { name, reason } => AdminCommand::RevokeCert {
            name,
            reason: reason.map(|r| match r.as_str() {
                "key_compromise" => RevokeReason::KeyCompromise,
                "superseded" => RevokeReason::Superseded,
                "cessation_of_operation" => RevokeReason::CessationOfOperation,
                _ => RevokeReason::Unspecified,
            }),
        },
        AdminCmd::Cert {
            cmd: CertCmd::Retry { name },
        } => AdminCommand::CertRetry { name },
        AdminCmd::Reload => AdminCommand::Reload,
        AdminCmd::BrakeStatus => AdminCommand::BrakeStatus,
    }
}

fn main() -> anyhow::Result<()> {
    // Several crates in the graph enable rustls with both aws-lc-rs and ring; pick one
    // process-wide provider before any TLS client or server config is built.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
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
        match &cli.cmd {
            Cmd::Keygen { out } => {
                let public = share_edge::keygen(out)?;
                println!("{public}");
                println!("z32: {}", public.to_z32());
                eprintln!("secret key written to {} (0600); keep it offline", out.display());
                return Ok(());
            }
            Cmd::Retire {
                standby_key: Some(standby_key),
                edge_key: Some(edge_key),
            } => {
                // Offline: the config is read only when present (its `standby_public` is checked).
                let cfg = if cli.config.exists() {
                    Some(Config::load(&cli.config)?.0)
                } else {
                    None
                };
                if let Some(cfg) = &cfg {
                    refuse_running(cfg).await?;
                }
                let signed = share_edge::retire_offline(
                    standby_key,
                    edge_key,
                    cfg.and_then(|c| c.standby_public),
                    share_edge::state::now_ms(),
                )?;
                println!("{}", serde_json::to_string(&signed)?);
                return Ok(());
            }
            _ => {}
        }
        let (cfg, secrets) = Config::load(&cli.config)?;
        match cli.cmd {
            Cmd::Run => share_edge::run(cfg, secrets, cli.config.clone()).await,
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
                } else if let Some(Ok(m)) = resp.result_as::<InviteMinted>() {
                    // The ticket with the invite, alone on the first line, for copy and paste.
                    println!("{}", m.ticket);
                    print!("{}", serde_yaml::to_string(&resp.result)?);
                } else {
                    print!("{}", serde_yaml::to_string(&resp.result)?);
                }
                if !resp.ok {
                    std::process::exit(1);
                }
                Ok(())
            }
            Cmd::Acme {
                cmd: AcmeCmd::Register { ca },
            } => {
                let (uri, lines) = share_edge::acme::register(&cfg, &secrets, &ca).await?;
                println!("account: {uri}");
                println!("Add these CAA records to {} before `run` (§8.2):", cfg.domain);
                for l in lines {
                    println!("{l}");
                }
                Ok(())
            }
            Cmd::Ticket { standby } => {
                let t = if standby {
                    share_edge::standby_ticket(&cfg)?
                } else {
                    let key = share_edge::load_or_create_key(&cfg.key_path())?;
                    share_edge::ticket(&cfg, key.public())?
                };
                println!("{t}");
                Ok(())
            }
            Cmd::Retire { standby_key, .. } => {
                refuse_running(&cfg).await?;
                let now = share_edge::state::now_ms();
                let signed = match standby_key {
                    Some(p) => share_edge::retire_offline(&p, &cfg.key_path(), cfg.standby_public, now)?,
                    None => share_edge::retire(&cfg, now)?,
                };
                println!("{}", serde_json::to_string(&signed)?);
                Ok(())
            }
            Cmd::Keygen { .. } => unreachable!("handled before the config is read"),
        }
    })
}

/// `retire` needs the edge stopped: refused while the admin socket answers.
async fn refuse_running(cfg: &Config) -> anyhow::Result<()> {
    if tokio::net::UnixStream::connect(cfg.admin_socket()).await.is_ok() {
        anyhow::bail!("the edge is running (admin socket answers): stop it before `retire`");
    }
    Ok(())
}
