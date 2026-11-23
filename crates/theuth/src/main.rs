//! `theuth`: build the runtime, start the plugin list, wait for shutdown.
//! Also the Python runtime worker: `t subprocess` (design doc 08 §9, [`worker`]).

mod bench_models;
mod link_cli;
mod logs;
mod mcp;
mod secret_cli;
mod service;
mod worker;
#[cfg(test)]
mod wire_discovery_tests;

use std::path::{Path, PathBuf};

use clap::Parser;
use figment::Figment;
use figment::providers::{Env, Format, Yaml};
use runtime::{Plugin, Runtime, SessionId};
use tracing_subscriber::fmt::writer::MakeWriterExt;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

/// The worker's memory limits are enforced by the allocator, so the process
/// that runs `t subprocess` must own it. Unarmed (every other mode) it is
/// `System` plus one live-byte counter.
#[global_allocator]
static ALLOC: monty_alloc::LimitedAllocator = monty_alloc::LimitedAllocator;

#[derive(Parser, Debug)]
#[command(name = "t", version, about = "what the harness")]
struct Cli {
    /// Data directory (config, secrets vault, mcp/, sqlite, logs, token, skills).
    #[arg(long, env = "THEUTH_DATA_DIR")]
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
    /// Speak the Agent Client Protocol on stdio (for editors); implies no TUI and no RPC server.
    #[arg(long, conflicts_with_all = ["headless", "serve", "prompt", "link_signal"])]
    acp: bool,
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
    /// Print the Slack app manifest (paste into api.slack.com/apps → Create from manifest) and exit.
    #[arg(long, conflicts_with_all = ["headless", "serve", "acp", "link_signal"])]
    slack_manifest: bool,
    /// Interactive Slack setup: create the app from the manifest (or update
    /// `--app`), verify the bot and app-level tokens, set the owner, write the
    /// tokens into the secrets vault and the owner into config.yaml, exit.
    #[arg(long, conflicts_with_all = ["headless", "serve", "acp", "link_signal", "slack_manifest", "telegram_setup", "prompt"])]
    slack_setup: bool,
    /// Existing Slack app id (`A…`) that --slack-setup updates to the manifest instead of creating one.
    #[arg(long, requires = "slack_setup")]
    app: Option<String>,
    /// Set up the Telegram bot: token into the vault, command menus, a pairing
    /// link, writes telegram.owner/enabled (design doc 98 §10.4).
    #[arg(long, conflicts_with_all = ["headless", "serve", "acp", "link_signal", "slack_manifest", "slack_setup", "prompt"])]
    telegram_setup: bool,
    /// Pair a phone with the running daemon (one-shot: prints a QR and the
    /// key, waits for the scan, exits). Needs `t --headless` on this box.
    #[arg(long, conflicts_with_all = ["headless", "serve", "acp", "prompt", "link_signal", "slack_manifest", "slack_setup", "telegram_setup"])]
    link: bool,
    /// Install this harness as a background service (launchd LaunchAgent or
    /// systemd user unit) that runs `t --headless` through the login shell,
    /// set `link.enabled: true`, start it, then pair it to your main harness
    /// as a node (design doc 32 §5.1).
    #[arg(long, conflicts_with_all = ["headless", "serve", "acp", "prompt", "link_signal", "slack_manifest", "slack_setup", "telegram_setup", "link", "resume", "uninstall_service"])]
    install_service: bool,
    /// Stop and remove the service unit written by --install-service. The data dir is untouched.
    #[arg(long, conflicts_with_all = ["headless", "serve", "acp", "prompt", "link_signal", "slack_manifest", "slack_setup", "telegram_setup", "link", "resume"])]
    uninstall_service: bool,
    /// Relay WebSocket URL for --install-service (`link.relay`); default: the configured relay.
    #[arg(long, requires = "install_service")]
    relay: Option<String>,
    /// Display name for --install-service (`link.name`); default: hostname.
    #[arg(long, requires = "install_service")]
    name: Option<String>,
    /// --install-service: skip the pairing step after the service starts.
    #[arg(long, requires = "install_service")]
    no_pair: bool,
    /// Store a secret: value read from the terminal without echo (asked twice),
    /// or the first line of stdin when piped. Exit 0 with `stored NAME vN`.
    #[arg(long, value_name = "NAME", conflicts_with_all = ["headless", "serve", "acp", "prompt", "resume", "link_signal", "slack_manifest", "slack_setup", "telegram_setup", "link", "install_service", "uninstall_service", "secret_rm", "secret_list"])]
    secret_set: Option<String>,
    /// Note shown in `secret_list` next to the name stored by --secret-set.
    #[arg(long, requires = "secret_set")]
    secret_note: Option<String>,
    /// Delete a secret and every version of it.
    #[arg(long, value_name = "NAME", conflicts_with_all = ["headless", "serve", "acp", "prompt", "resume", "link_signal", "slack_manifest", "slack_setup", "telegram_setup", "link", "install_service", "uninstall_service", "secret_list"])]
    secret_rm: Option<String>,
    /// List secrets: names, versions, notes, dates. Never values.
    #[arg(long, conflicts_with_all = ["headless", "serve", "acp", "prompt", "resume", "link_signal", "slack_manifest", "slack_setup", "telegram_setup", "link", "install_service", "uninstall_service"])]
    secret_list: bool,
    /// Check the release manifest once, verify, replace this binary on disk
    /// (or `brew upgrade theuth` for a Homebrew install), print one line, exit (39 §3.2).
    #[arg(long, conflicts_with_all = ["headless", "serve", "acp", "prompt", "resume", "link_signal", "slack_manifest", "slack_setup", "telegram_setup", "link", "install_service", "uninstall_service", "secret_set", "secret_rm", "secret_list"])]
    update: bool,
    /// With `--update`: stop the user cells running on this data dir's engine instead of
    /// refusing when no terminal can ask `wait / stop / cancel` (171 §5.3 F6).
    #[arg(long, requires = "update")]
    stop_cells: bool, // 171:A1
    /// Print the app library compatibility hash this binary was built with and
    /// exit (46 §2.1): the updater checks a staged `theuth-app-parser-<hash>` helper
    /// against it. Before config, logging or any plugin.
    #[arg(long, conflicts_with_all = ["headless", "serve", "acp", "prompt", "resume", "link_signal", "slack_manifest", "slack_setup", "telegram_setup", "link", "install_service", "uninstall_service", "secret_set", "secret_rm", "secret_list", "update"])]
    app_parser_hash: bool,
    /// 57 §11.4: fold the captured training rows of a decision family prefix (`meeting`) into
    /// the phrasing-bench fixture shape (or laya's typed-decisions rows with --format laya),
    /// print (or --out) and exit. Reads <data_dir>/decisions/training/<prefix>/*.jsonl.
    #[arg(long, value_name = "PREFIX", conflicts_with_all = ["headless", "serve", "acp", "prompt", "resume", "link_signal", "slack_manifest", "slack_setup", "telegram_setup", "link", "install_service", "uninstall_service", "secret_set", "secret_rm", "secret_list", "update"])]
    decision_export: Option<String>,
    /// --decision-export: only day files from this date on (YYYY-MM-DD).
    #[arg(long, requires = "decision_export", value_name = "YYYY-MM-DD")]
    since: Option<chrono::NaiveDate>,
    /// --decision-export: write here instead of stdout.
    #[arg(long, requires = "decision_export", value_name = "FILE")]
    out: Option<PathBuf>,
    /// --decision-export: `fixture` (default) | `laya`.
    #[arg(long, requires = "decision_export", default_value = "fixture")]
    format: String,
    /// 58 step 12: load ONNX Runtime and print the execution provider sessions get, run the
    /// STT encoder once on 5 s of silence, open the decision checkpoint and run one question,
    /// each with its ms and device; exit 2 naming what is missing. `$ORT_DYLIB_PATH` honoured.
    #[arg(long, conflicts_with_all = ["headless", "serve", "acp", "prompt", "resume", "link_signal", "slack_manifest", "slack_setup", "telegram_setup", "link", "install_service", "uninstall_service", "secret_set", "secret_rm", "secret_list", "update", "decision_export"])]
    bench_models: bool,
    /// --bench-models: override `models.accelerator` (`auto` | `cpu` | `webgpu` | `directml` | `cuda`).
    #[arg(long, requires = "bench_models", value_name = "ACCEL")]
    accelerator: Option<models::accelerator::Accelerator>,
    /// 67: `t mesh …` / `t tasks …` — RPC clients of the running engine.
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

/// Subcommands (67 §4.1, §12; 79 §8). Each connects to the running engine and
/// calls one RPC method (`mesh add --ssh|--docker` runs bring-up here and joins
/// over RPC; `--vm` is a pure RPC client of `sandbox_create`).
#[derive(clap::Subcommand, Debug)]
enum Cmd {
    /// The engine mesh: peers, pairing, the hub lease (design doc 67 §4).
    Mesh {
        #[command(subcommand)]
        cmd: MeshCmd,
    },
    /// The task board (design doc 67 §7).
    Tasks {
        #[command(subcommand)]
        cmd: TasksCmd,
    },
    /// One session's diagnostics.
    Session {
        #[command(subcommand)]
        cmd: SessionCmd,
    },
    /// Sandbox VMs on this engine (design doc 79 §8): list, start, stop, remove, desktop URL.
    Sandbox {
        #[command(subcommand)]
        cmd: SandboxCmd,
    },
    /// An MCP server on stdio. With `--session` (75 §9): the shim an ACP
    /// child gets in `session/new.mcpServers` (`list_skills`, `load_skill`,
    /// `send_message`, and 193 §7's `read_file`, `edit`, `move_text`,
    /// `copy_text`, `indent`, `clip`). Without (158 §12.3): theuth's data
    /// catalogue — the same tools the engine serves over HTTP — for any MCP
    /// client's `mcpServers` entry. Both need a running engine.
    Mcp {
        /// The child's theuth session id (ACP shim mode).
        #[arg(long)]
        session: Option<String>,
    },
    /// The owner's double (design doc 126): the Tier B training job a GPU node runs in-process
    /// (`t double train-b`), without an engine.
    Double {
        #[command(subcommand)]
        cmd: DoubleCmd,
    },
    /// The nightly dreaming routine (design doc 136): offline jobs over a data dir, no engine needed.
    Dreaming {
        #[command(subcommand)]
        cmd: DreamingCmd,
    },
}

/// `t dreaming …` (136 §3.3 item 7).
#[derive(clap::Subcommand, Debug)]
enum DreamingCmd {
    /// Recall plus the rule-only parts of Tidy over `--data-dir` (its `knowledge/`,
    /// `knowledge.sqlite`, `sessions.sqlite`) with zero writes; prints the counts the
    /// night's journal would show. Run it against a copy of a live data dir.
    DryRun {
        /// The data dir to read; defaults to the global `--data-dir`.
        #[arg(long)]
        data_dir: Option<PathBuf>,
        /// Print the report as one JSON object instead of text.
        #[arg(long)]
        json: bool,
    },
}

/// `t double …` (126 §8.4, Revision 2): one-shot jobs, no running engine needed.
#[derive(clap::Subcommand, Debug)]
enum DoubleCmd {
    /// Train a Tier B LoRA adapter from a bundle dir (`manifest.json` + `rows.jsonl`) into
    /// `--out` (`adapter.safetensors` + `manifest.json`); progress on stdout; exit 0 when the
    /// candidate dir is populated. Needs a `t` built with `--features decision/train`
    /// (and `decision/cuda` on a CUDA node). The checkpoint is `decision.checkpoints.default`
    /// under `--data-dir`, fetched by the models downloader beforehand.
    TrainB {
        /// The bundle dir.
        #[arg(long)]
        bundle: PathBuf,
        /// The candidate version dir (a `checkpoint/` inside it resumes a paused run).
        #[arg(long)]
        out: PathBuf,
        /// `auto` (the `decision.device` / `models.accelerator` rule) | `metal` | `cuda` | `cpu`.
        #[arg(long, default_value = "auto")]
        device: String,
    },
}

#[derive(clap::Subcommand, Debug)]
enum MeshCmd {
    /// This engine, its peers and who holds the hub lease.
    Status,
    /// Print a pairing string for `t mesh add` on another engine.
    Pair,
    /// Pair with the engine behind a pairing string (mutual and transitive) —
    /// or make one first: `--ssh user@host [--key NAME | --system-ssh]` installs
    /// `t` over SSH, `--docker [--name N] [--image I | --local-t PATH]` runs it
    /// in a container here, `--vm [--name N] [--cpus N] [--memory 4G] [--disk 20G]
    /// [--share HOST[:GUEST][:ro]]… [--no-desktop] [--network public,host[,private]]
    /// [--image REF | --bootstrap] [--local-t PATH]` makes a sandbox VM (design
    /// doc 79); all end paired (design doc 67 §13).
    Add {
        /// The string `t mesh pair` printed on the other engine.
        #[arg(required_unless_present_any = ["ssh", "docker", "vm"])]
        pairing: Option<String>,
        /// Make an engine on this SSH host (`[user@]host[:port]`, or an `~/.ssh/config` alias with --system-ssh).
        #[arg(long, value_name = "TARGET", conflicts_with_all = ["pairing", "docker", "vm"])]
        ssh: Option<String>,
        /// Vault name of the private key for the built-in SSH client (store it with `t --secret-set NAME < key`).
        #[arg(
            long,
            value_name = "NAME",
            requires = "ssh",
            conflicts_with = "system_ssh"
        )]
        key: Option<String>,
        /// Use the system `ssh` binary (your `~/.ssh/config`, agent, ProxyJump) instead of the built-in client.
        #[arg(long, requires = "ssh")]
        system_ssh: bool,
        /// Make an engine in a Docker container on this machine.
        #[arg(long, conflicts_with_all = ["pairing", "ssh", "vm"])]
        docker: bool,
        /// Make a sandbox VM on this machine (an isolated Linux desktop, design doc 79); the engine does the work.
        #[arg(long, conflicts_with_all = ["pairing", "ssh", "docker"])]
        vm: bool,
        /// Engine and record name; default: the SSH host, `engine` for docker, `vm-1` for a VM.
        #[arg(long, value_name = "N")]
        name: Option<String>,
        /// --docker: a pre-built image ref instead of building deploy/computer/Dockerfile; --vm: the OCI image.
        #[arg(long, value_name = "I", conflicts_with_all = ["local_t", "bootstrap", "ssh"])]
        image: Option<String>,
        /// --docker / --vm: a locally built Linux `t` to install instead of the image's.
        #[arg(long, value_name = "PATH", conflicts_with = "ssh")]
        local_t: Option<PathBuf>,
        /// --vm: vCPUs (default from the host).
        #[arg(long, requires = "vm")]
        cpus: Option<u8>,
        /// --vm: memory, e.g. `4G` or `4096M` (default from the host).
        #[arg(long, requires = "vm", value_name = "SIZE")]
        memory: Option<String>,
        /// --vm: root disk, e.g. `20G` (default 20G).
        #[arg(long, requires = "vm", value_name = "SIZE")]
        disk: Option<String>,
        /// --vm: share a host directory: `HOST[:GUEST][:ro]` (repeatable).
        #[arg(long, requires = "vm", value_name = "HOST[:GUEST][:ro]")]
        share: Vec<String>,
        /// --vm: no desktop (no VNC, no XFCE).
        #[arg(long, requires = "vm")]
        no_desktop: bool,
        /// --vm: network policy `public,host` (default) | `public,host,private` | `public` | `none`.
        #[arg(long, requires = "vm", value_name = "POLICY")]
        network: Option<String>,
        /// --vm: our `theuth-vm-base:trixie` systemd base plus a first-boot install script instead of the published image.
        #[arg(long, requires = "vm")]
        bootstrap: bool,
    },
    /// Leave an engine out of the mesh.
    Remove {
        /// Engine id (see `t mesh status`).
        id: String,
    },
    /// This engine's willingness to hold the hub lease.
    Hub {
        /// prefer | eligible | never
        pref: String,
    },
    /// Devices paired to this mesh (design doc 69): list, pair, revoke, rename.
    Devices {
        #[command(subcommand)]
        cmd: DevicesCmd,
    },
}

#[derive(clap::Subcommand, Debug)]
enum DevicesCmd {
    /// Every device in the replicated registry, revoked ones included.
    List,
    /// Start a pairing: prints the QR and the string a device scans (5 min).
    Add {
        /// The name the QR shows; default: the mesh's name.
        name: Option<String>,
    },
    /// Revoke a device: refused by every hub within one replication hop.
    Revoke {
        /// Device id (see `list`).
        id: String,
    },
    /// Rename a device.
    Rename {
        /// Device id (see `list`).
        id: String,
        /// New name.
        name: String,
    },
}

#[derive(clap::Subcommand, Debug)]
enum TasksCmd {
    /// Every task, oldest first; `--owner agent:<name>|user`, `--state todo,doing,…`, `--engine`, `--project`.
    List {
        /// `agent:<name>` | `user`.
        #[arg(long)]
        owner: Option<String>,
        /// Comma-separated states.
        #[arg(long, value_delimiter = ',')]
        state: Option<Vec<String>>,
        /// Engine name (`where`).
        #[arg(long)]
        engine: Option<String>,
        /// Project label.
        #[arg(long)]
        project: Option<String>,
    },
}

#[derive(clap::Subcommand, Debug)]
enum SessionCmd {
    /// Where the session's frame tokens go (70 COMPACT-SIZE): head images/text,
    /// tail, system, the calibrated estimate the gauge shows after a
    /// compaction, and the provider's last billed context.
    Context {
        /// Session id.
        session: String,
    },
}

#[derive(clap::Subcommand, Debug)]
enum SandboxCmd {
    /// Every sandbox with state, engine and resources.
    Ls,
    /// Boot a stopped VM.
    Start {
        /// Record name.
        name: String,
    },
    /// Stop a VM gracefully (refuses while sessions run unless --force).
    Stop {
        /// Record name.
        name: String,
        /// Stop even with running sessions.
        #[arg(long)]
        force: bool,
    },
    /// Destroy a VM: peer, VM, disk, vault row, record.
    Rm {
        /// Record name.
        name: String,
        /// Keep its snapshots (default: keep).
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        keep_snapshots: bool,
    },
    /// Print the noVNC URL and password for a browser.
    Desktop {
        /// Record name.
        name: String,
    },
}

/// The running engine's RPC address per `engine.json`, else the configured one.
fn listen(data_dir: &Path, config: &Figment) -> String {
    rpc::engine::discover(data_dir)
        .and_then(|r| r.record.listen)
        .unwrap_or_else(|| link_cli::listen(config))
}

/// `t mesh …` / `t tasks …`: one RPC call on the running engine, its JSON
/// reply pretty-printed; the server's error message (today `not built`) on
/// stderr, exit 1. No daemon → the `t --link` hint, exit 1.
async fn cmd_67(data_dir: &Path, config: &Figment, cmd: Cmd) -> anyhow::Result<()> {
    use jsonrpsee::core::ClientError;
    use jsonrpsee::core::client::ClientT;
    use jsonrpsee::rpc_params;

    let listen = listen(data_dir, config);
    let Ok(client) = link_cli::connect(data_dir, &listen).await else {
        eprintln!("no harness on {listen}; start `t --headless` first");
        std::process::exit(1);
    };
    let r: Result<serde_json::Value, ClientError> = match cmd {
        Cmd::Mesh { cmd } => match cmd {
            MeshCmd::Status => client.request("mesh_status", rpc_params![]).await,
            MeshCmd::Pair => client.request("mesh_pairing_string", rpc_params![]).await,
            MeshCmd::Add {
                pairing: Some(pairing),
                ..
            } => client.request("mesh_add", rpc_params![pairing]).await,
            MeshCmd::Add {
                pairing: None,
                vm: true,
                name,
                image,
                local_t,
                cpus,
                memory,
                disk,
                share,
                no_desktop,
                network,
                bootstrap,
                ..
            } => {
                let spec = vm_spec(
                    name, image, local_t, cpus, memory, disk, share, no_desktop, network, bootstrap,
                )?;
                return sandbox_create(client, spec).await;
            }
            MeshCmd::Add {
                pairing: None,
                ssh,
                key,
                system_ssh,
                name,
                image,
                local_t,
                ..
            } => {
                let target = match ssh {
                    Some(target) => sandbox::Target::Ssh {
                        target,
                        key,
                        system: system_ssh,
                        name,
                    },
                    None => sandbox::Target::Docker {
                        name: name.unwrap_or_else(|| "engine".into()),
                        image,
                        local_t,
                    },
                };
                return make_engine(data_dir, config, client, target).await;
            }
            MeshCmd::Remove { id } => client.request("mesh_remove", rpc_params![id]).await,
            MeshCmd::Hub { pref } => {
                let pref: mesh::HubPref =
                    serde_json::from_value(serde_json::Value::String(pref))
                        .map_err(|_| anyhow::anyhow!("hub: expected prefer|eligible|never"))?;
                client.request("mesh_set_hub", rpc_params![pref]).await
            }
            MeshCmd::Devices { cmd } => match cmd {
                DevicesCmd::List => client.request("devices_list", rpc_params![]).await,
                DevicesCmd::Add { name } => {
                    let r: Result<link::PairingPayload, ClientError> =
                        client.request("device_pair_start", rpc_params![name]).await;
                    match r {
                        Ok(p) => {
                            println!(
                                "{}\n{}\npending {} — expires in 5 min",
                                link::qr(&p.string),
                                p.string,
                                p.pending_id
                            );
                            return Ok(());
                        }
                        Err(e) => Err(e),
                    }
                }
                DevicesCmd::Revoke { id } => client.request("device_revoke", rpc_params![id]).await,
                DevicesCmd::Rename { id, name } => {
                    client.request("device_rename", rpc_params![id, name]).await
                }
            },
        },
        Cmd::Tasks { cmd } => match cmd {
            TasksCmd::List {
                owner,
                state,
                engine,
                project,
            } => {
                let state = state
                    .map(|s| {
                        s.iter()
                            .map(|v| serde_json::from_value(serde_json::Value::String(v.clone())))
                            .collect::<Result<Vec<tasks::TaskState>, _>>()
                    })
                    .transpose()
                    .map_err(|_| {
                        anyhow::anyhow!("state: expected todo|doing|review|blocked|done|cancelled")
                    })?;
                let filter = tasks::TaskFilter {
                    owner,
                    state,
                    engine,
                    project,
                    ..Default::default()
                };
                client.request("tasks_list", rpc_params![filter]).await
            }
        },
        Cmd::Session { cmd } => match cmd {
            SessionCmd::Context { session } => {
                client
                    .request("session_context_breakdown", rpc_params![session])
                    .await
            }
        },
        Cmd::Sandbox { cmd } => match cmd {
            SandboxCmd::Ls => {
                let r: Result<Vec<sandbox::SandboxInfo>, ClientError> =
                    client.request("sandboxes_list", rpc_params![]).await;
                match r {
                    Ok(list) if list.is_empty() => {
                        println!("no sandboxes");
                        return Ok(());
                    }
                    Ok(list) => {
                        for i in &list {
                            println!("{}", sandbox_line(i));
                        }
                        return Ok(());
                    }
                    Err(e) => Err(e),
                }
            }
            SandboxCmd::Start { name } => client.request("sandbox_start", rpc_params![name]).await,
            SandboxCmd::Stop { name, force } => {
                client
                    .request("sandbox_stop", rpc_params![name, 20u64, force])
                    .await
            }
            SandboxCmd::Rm {
                name,
                keep_snapshots,
            } => {
                client
                    .request("sandbox_destroy", rpc_params![name, keep_snapshots])
                    .await
            }
            SandboxCmd::Desktop { name } => {
                let r: Result<sandbox::types::DesktopAccess, ClientError> = client
                    .request("sandbox_desktop_access", rpc_params![name])
                    .await;
                match r {
                    Ok(d) => {
                        println!(
                            "{}\npassword: {}\ngeometry: {}",
                            d.http_url, d.password, d.geometry
                        );
                        return Ok(());
                    }
                    Err(e) => Err(e),
                }
            }
        },
        // Dispatched by `run` before this: stdout is its protocol channel.
        Cmd::Mcp { .. } => unreachable!("t mcp is served by mcp::run"),
        Cmd::Double { .. } => unreachable!("t double runs without an engine"),
        Cmd::Dreaming { .. } => unreachable!("t dreaming runs without an engine"),
    };
    match r {
        Ok(v) => {
            println!("{}", serde_json::to_string_pretty(&v)?);
            Ok(())
        }
        Err(ClientError::Call(e)) => {
            eprintln!("{}", e.message());
            std::process::exit(1);
        }
        Err(e) => Err(e.into()),
    }
}

/// `t mesh add --ssh|--docker`: bring-up runs in this process (progress on
/// stdout, the SSH key read from the vault here — the same in-process open
/// `--secret-set` uses), and the two mesh steps go to the running engine over
/// RPC like every other `t mesh` call.
async fn make_engine(
    data_dir: &Path,
    config: &Figment,
    client: jsonrpsee::ws_client::WsClient,
    target: sandbox::Target,
) -> anyhow::Result<()> {
    use jsonrpsee::core::client::ClientT;
    use jsonrpsee::rpc_params;

    struct RpcJoin(jsonrpsee::ws_client::WsClient);

    #[async_trait::async_trait]
    impl sandbox::MeshJoin for RpcJoin {
        async fn add_peer(&self, pairing: String) -> anyhow::Result<mesh::Peer> {
            Ok(self.0.request("mesh_add", rpc_params![pairing]).await?)
        }
        async fn status(&self) -> anyhow::Result<mesh::MeshStatus> {
            Ok(self.0.request("mesh_status", rpc_params![]).await?)
        }
        async fn remove_peer(&self, id: mesh::EngineId) -> anyhow::Result<()> {
            Ok(self.0.request("mesh_remove", rpc_params![id.0]).await?)
        }
    }

    let cfg: secrets::SecretsConfig = if config.contains("secrets") {
        config.extract_inner("secrets")?
    } else {
        secrets::SecretsConfig::default()
    };
    let secret = move |name: String| async move {
        let vault = secrets::Vault::open(data_dir, cfg.key_store)
            .await
            .map_err(|e| format!("vault: {e:#}"))?;
        vault.get(&name, 0).ok_or_else(|| {
            format!("no secret named {name}; store the key with `t --secret-set {name} < key`")
        })
    };
    match sandbox::make_engine(
        data_dir,
        target,
        secret,
        &RpcJoin(client),
        &sandbox::Progress::stdout(),
        sandbox::Cmd::DEFAULT_TIMEOUT,
    )
    .await
    {
        Ok(peer) => {
            println!("engine {} ({}) joined the mesh", peer.name, peer.id);
            Ok(())
        }
        Err(e) => {
            eprintln!("{e:#}");
            std::process::exit(1);
        }
    }
}

/// `4G` | `4096M` | `4096` (MiB) → MiB.
fn parse_mib(s: &str) -> anyhow::Result<u32> {
    let t = s.trim();
    let (num, mul) = match t.chars().last() {
        Some('G' | 'g') => (&t[..t.len() - 1], 1024),
        Some('M' | 'm') => (&t[..t.len() - 1], 1),
        _ => (t, 1),
    };
    let n: u32 = num
        .trim()
        .parse()
        .map_err(|_| anyhow::anyhow!("{s}: expected a size like 4G or 4096M"))?;
    Ok(n * mul)
}

/// `HOST[:GUEST][:ro]` → a mount.
fn parse_share(s: &str) -> anyhow::Result<sandbox::Mount> {
    let mut parts = s.split(':');
    let host = parts
        .next()
        .filter(|h| !h.is_empty())
        .ok_or_else(|| anyhow::anyhow!("--share: empty host path"))?;
    let mut guest = String::new();
    let mut readonly = false;
    for p in parts {
        match p {
            "ro" => readonly = true,
            "rw" => readonly = false,
            g if g.starts_with('/') && guest.is_empty() => guest = g.to_owned(),
            other => anyhow::bail!("--share {s}: unexpected part {other:?}"),
        }
    }
    Ok(sandbox::Mount {
        host: host.into(),
        guest,
        readonly,
    })
}

/// The `t mesh add --vm` flags as a `SandboxCreate`.
#[allow(clippy::too_many_arguments)]
fn vm_spec(
    name: Option<String>,
    image: Option<String>,
    local_t: Option<PathBuf>,
    cpus: Option<u8>,
    memory: Option<String>,
    disk: Option<String>,
    share: Vec<String>,
    no_desktop: bool,
    network: Option<String>,
    bootstrap: bool,
) -> anyhow::Result<sandbox::SandboxCreate> {
    let network = network
        .map(|n| {
            let n = n.replace(' ', "");
            let v = match n.as_str() {
                "public,host" | "host,public" => "public_host",
                "public,host,private" | "public,private,host" | "host,public,private" => {
                    "public_host_private"
                }
                "public" => "public_only",
                "none" => "none",
                other => anyhow::bail!(
                    "--network {other}: expected public,host | public,host,private | public | none"
                ),
            };
            Ok::<_, anyhow::Error>(serde_json::from_value::<sandbox::NetPolicy>(
                serde_json::Value::String(v.into()),
            )?)
        })
        .transpose()?;
    let image = match (image, bootstrap) {
        (Some(r), _) => Some(sandbox::ImageRef::Oci { r#ref: r }),
        (None, true) => Some(sandbox::ImageRef::Bootstrap {
            base: "ghcr.io/basedatum/theuth-vm-base:trixie".into(),
            script: "start-desktop".into(),
        }),
        (None, false) => None,
    };
    Ok(sandbox::SandboxCreate {
        name: name.unwrap_or_else(|| "vm-1".into()),
        purpose: None,
        cpus,
        memory_mib: memory.as_deref().map(parse_mib).transpose()?,
        root_disk_mib: disk.as_deref().map(parse_mib).transpose()?,
        max_cpus: None,
        max_memory_mib: None,
        shares: share
            .iter()
            .map(|s| parse_share(s))
            .collect::<Result<_, _>>()?,
        desktop: Some(!no_desktop),
        geometry: None,
        network,
        image,
        autostart: None,
        local_t,
    })
}

/// One line of `t sandbox ls`.
fn sandbox_line(i: &sandbox::SandboxInfo) -> String {
    let state = serde_json::to_value(&i.state)
        .ok()
        .and_then(|v| v.get("state").and_then(|s| s.as_str()).map(str::to_owned))
        .unwrap_or_default();
    let mut s = format!(
        "{:<16} {:<7} {:<10} engine={}",
        i.name,
        i.backend,
        state,
        i.engine.as_deref().unwrap_or("-")
    );
    if let Some(vm) = &i.vm {
        let r = vm.resources;
        s.push_str(&format!(
            " cpus={} memory={}M disk={}M shares={} desktop={}",
            r.cpus,
            r.memory_mib,
            r.root_disk_mib,
            vm.shares.len(),
            if vm.desktop.is_some() { "yes" } else { "no" }
        ));
    }
    s
}

/// `t mesh add --vm`: `sandbox_create` over RPC, then print `Progress` lines
/// from `subscribe_events` until the record is `ready` or `failed`. Preflight
/// blockers arrive as the server's error message, one sentence per line.
async fn sandbox_create(
    client: jsonrpsee::ws_client::WsClient,
    spec: sandbox::SandboxCreate,
) -> anyhow::Result<()> {
    use jsonrpsee::core::ClientError;
    use jsonrpsee::core::client::{ClientT, Subscription, SubscriptionClientT};
    use jsonrpsee::rpc_params;

    // Subscribe first so no progress line is missed.
    let mut events: Subscription<serde_json::Value> = client
        .subscribe(
            "subscribe_events",
            rpc_params![serde_json::Value::Null, serde_json::Value::Null],
            "unsubscribe_events",
        )
        .await?;
    let name = spec.name.clone();
    let r: Result<sandbox::SandboxInfo, ClientError> =
        client.request("sandbox_create", rpc_params![spec]).await;
    match r {
        Ok(_) => println!("[create] {name}: creating"),
        Err(ClientError::Call(e)) => {
            eprintln!("cannot create {name}:\n{}", e.message());
            std::process::exit(1);
        }
        Err(e) => return Err(e.into()),
    }
    while let Some(ev) = events.next().await {
        let ev = ev?;
        let of_ours = ev.get("name").and_then(|n| n.as_str()) == Some(name.as_str())
            || ev.pointer("/sandbox/name").and_then(|n| n.as_str()) == Some(name.as_str());
        if !of_ours {
            continue;
        }
        match ev.get("type").and_then(|t| t.as_str()) {
            Some("Progress") => println!(
                "[{}] {}",
                ev.get("step").and_then(|s| s.as_str()).unwrap_or("-"),
                ev.get("line").and_then(|l| l.as_str()).unwrap_or("")
            ),
            Some("Changed") => match ev.pointer("/sandbox/state/state").and_then(|s| s.as_str()) {
                Some("ready") | Some("running") => {
                    let engine = ev
                        .pointer("/sandbox/engine")
                        .and_then(|e| e.as_str())
                        .unwrap_or("-");
                    println!("engine {name} ({engine}) joined the mesh");
                    return Ok(());
                }
                Some("failed") => {
                    eprintln!(
                        "{name} failed at {}: {}\nretry from Settings › Mesh or `t sandbox rm {name}`",
                        ev.pointer("/sandbox/state/step")
                            .and_then(|s| s.as_str())
                            .unwrap_or("?"),
                        ev.pointer("/sandbox/state/error")
                            .and_then(|s| s.as_str())
                            .unwrap_or("?")
                    );
                    std::process::exit(1);
                }
                _ => {}
            },
            _ => {}
        }
    }
    anyhow::bail!("the engine closed the event stream before {name} was ready")
}

/// 183 §6.1: the `communication:` line first runs before 183 wrote. A config still holding it
/// byte for byte gets [`COMMUNICATION_LINE`] instead, once, so an untouched config takes the
/// mesh default; any edited value is the user's and stays.
const OLD_COMMUNICATION_LINE: &str = "  communication: false       # model-to-model communication (design doc 50): spawn_subagent, discover_peers, send_message.";
/// What [`DEFAULT_CONFIG`] writes now (commented out: absent = on in a mesh, else off).
const COMMUNICATION_LINE: &str = "  # communication: false     # model-to-model communication (design doc 50): spawn_subagent, discover_peers, send_message. Unset: on once this machine is paired into a mesh (183), else off.";

const DEFAULT_CONFIG: &str = "# theuth configuration. Every plugin reads its own section.
# Env overrides: THEUTH_<SECTION>__<KEY>, e.g. THEUTH_RPC__LISTEN=127.0.0.1:7799

agent:
  model: anthropic/claude-sonnet-4-5
  # communication: false     # model-to-model communication (design doc 50): spawn_subagent, discover_peers, send_message. Unset: on once this machine is paired into a mesh (183), else off.
                             # Off is a ceiling no session can override; `/communication on always` writes true here.
                             # `/communication off` in a session is memory-only and ends with the process.
  # title_model: anthropic/claude-haiku-4-5   # names sessions in the history list (07 §15); default: the session's model
  # vision_model: anthropic/claude-haiku-4-5  # answers the main model's questions about images it cannot see (52 §3);
                             # unset: no fallback. Must be an exact catalog `provider/model` that accepts images;
                             # images then go to that provider, with extra calls. /vision or Settings → Agent loop
  # escalate_model: anthropic/claude-haiku-4-5  # tier 2 of the local decision oracle (57 §0.9): one cheap no-tools call answers a typed question the oracle abstained on;
                             # unset: inherits title_model, then the session's model. Must be a catalog `provider/model`. /escalate or Settings → Agent loop
  # stream_idle_secs: 120     # seconds without a stream event before a model call is abandoned (36 §2.3); Settings → Agent loop
  # max_salvages: 2           # interrupted-reply recoveries per turn (stalled stream or output cap) before the turn errors (36 §2.4); 0 = off
  # aws_profile: work          # Bedrock inference for every session; /aws or Settings → Connections
  # openrouter:               # built-in OpenRouter provider (85 §0.6); key = secret `openrouter` or OPENROUTER_API_KEY
  #   provider: {order: [anthropic], allow_fallbacks: false}  # OpenRouter `provider` preferences, sent verbatim when set
  #   referer: https://theuth.io   # HTTP-Referer override; X-OpenRouter-Title via `title`
  # subagents:
  #   model: null              # subagent default model when spawn_subagent omits `model`, named in the system prompt; default: agent.model. /subagent or Settings → Chat → Models
  #   max_depth: 1
  #   max_concurrent: 16       # native child spawn turns at once per orchestrator tree; further spawns queue and the parent is told
  #   max_concurrent_external: 32  # external (ACP harness / mesh-node) child spawn turns at once per orchestrator tree; a separate pool
  #   max_runtime_secs: 600
  #   reply_timeout_secs: 120
  # knowledge:                # loop policy (design doc 10 §5); the store itself is `knowledge:` below
  #   hint_limit: 5            # lines per <knowledge> note
  #   hint_rounds: 4           # mid-turn hint cadence in rounds
  #   harvest_every: 3         # tool-using root turns per harvest; 0 disables harvest
  #   harvest_max: 8           # documents accepted per harvest

# knowledge:                 # design doc 10; the bundle lives in <data_dir>/knowledge/
#   enabled: true            # false: no store, no knowledge_* tools, no hints, no harvest
#   lifetimes:               # 136 §5.2, days: a `record` past its lifetime drops out of hints (dreaming archives it);
#     record: 14             # `fact`/`procedure`/`entity` lifetimes pace dreaming's verification sample
#     fact: 30
#     procedure: 90
#     entity: 60

# documents:                 # design doc 59; Yjs documents under <data_dir>/documents/, the BlockNote editor
#   enabled: true            # false: no store, no helper, no doc_* methods; concepts open as raw text
#   export_debounce_ms: 400  # settle time before the derived .md is rewritten (hard cap 3 s)
#   export_quiet_secs: 60    # no typing for this long before the owner records the edit (knowledge commit, artifact version);
#                            # flushed at once when the document closes, its last reader leaves, or theuth stops
#   max_doc_bytes: 8388608   # an update that would grow a document past this is refused (DocTooLarge)
#   snapshots_keep: 30       # automatic (daily) snapshots kept per document; labelled ones are never pruned
#   agent_color: '#7c3aed'  # the agent's presence and typewriter reveal colour in every editor
#   max_members: 64          # members per shared document; the 65th join is refused
#   sharing: on              # off: no new share links, members still connect; disabled: members refused too
#   idle_unload_secs: 600    # unload an unsubscribed document after this idle time (log compacted into snapshot.yjs)
#   max_open_docs: 256       # documents held in memory at most; the longest idle is unloaded past it
#                            # guest handshakes are rated under `link.guest_handshakes_per_minute` (10)

# surface:                   # Mermaid / json-render blocks in replies (design doc 30)
#   enabled: true            # false: no prompt block, no rendering; fences stay code blocks
#   correct: true            # <surface_error> fix-up rounds when a json-render/mermaid/openui block is invalid
#   correct_rounds: 3        # rounds per turn before the model is told to move on

# advisor:                   # a second model that reviews one session's transcript (design doc 125); never on by default —
#                            # the user turns it on per session with the model picker (/advisor, Advisor row). The pick is remembered here:
#   model: anthropic/claude-haiku-4-5   # written by the picker; pins it and serves `/advisor on`. Settings → Advisor
#   effort: null             # the pick's effort
#   max_notes_per_update: 4  # non-blocker notes accepted per update (1–32)
#   immune_turns: 3          # agent steps after a steer during which a concern is only an aside (0–10; 0 = off)
#   sync_backlog: off        # off | 1 | 3 | 5: pending reviews the turn end waits for (≤ 30 s), never while the advisor is failing
#   max_consults: 6          # `consult` calls the advised model (and its native subagents) may make per user turn (0–32; 0 removes the tool)

# web:                       # search_web / read_url (design doc 16); Settings > Search edits the read/budget keys live
#   enabled: true
#   engines: [google, brave, qwant, duckduckgo, wikipedia, arxiv, github]   # keyless built-ins; `bing` is opt-in (serves flagged clients empty pages)
#   fetch_max_chars: 4000
#   read_top: 5              # search_web reads this many top hits
#   read_chars: 2500         # passage budget per read page
#   read_budget_ms: 6000     # whole read phase; pages still loading are `skipped: budget`
#   render_cap_ms: 3000      # one browser-rendered page inside a search
#   recent_default: null     # day | week | month | year applied when a call passes none
#   keyless_fallback: true   # false: built-ins run only when no external provider serves the category
#   degraded_is_error: false # true: a degraded search (no external, <= 1 general engine) is an error result
#   allow_private_network: true    # read_url / http_request / scrape_url / crawl / map_site / site_*: false blocks
#                            # localhost / RFC1918 / CGNAT / IPv6 ULA and link-local / .local again; cloud metadata
#                            # (169.254/16 …) and restricted contacts never reach it. Settings > Network > Local network access

# scrape:                    # scrape_url / crawl / map_site (design doc 38)
#   concurrency: 5
#   max_pages: 50
#   max_depth: 3
#   same_domain: true
#   page_timeout_ms: 8000
#   run_timeout_ms: 60000
#   robots: true
#   delay_ms: 100            # per-host gap between requests; a larger robots Crawl-delay wins

# codeindex:                 # design doc 60; indexes live in <data_dir>/codeindex/<slug>-<hash>.sqlite
#   enabled: true            # false: no plugin, no code_* functions, nothing on disk
#   sync_limit: 64           # changed files re-indexed synchronously before answering; more → answer stale, finish in background
#   max_file_bytes: 1048576  # larger files are listed, not parsed
#   cochange_commits: 300    # first-parent commits walked for co-change; 0 disables the git walk
#   languages: [rust, typescript, javascript, python, go]   # subset to parse; others are files only

# search_providers:          # optional external engines registered with `web` (design doc 16 §3, §8); the
#                            # built-in keyless engines above need none of these. Settings > Search edits this live.
#   firecrawl:               # inert until its secret exists (`t --secret-set firecrawl` or Settings > Search)
#     enabled: true
#     base_url: https://api.firecrawl.dev
#     secret: firecrawl      # vault key
#     timeout_ms: 8000
#     priority: 30           # registration order among external providers, lower first
#   searxng:                 # a SearXNG instance you run (json format enabled)
#     base_url: http://127.0.0.1:8080
#   openserp:                # a karust/openserp server (docker: karust/openserp serve)
#     base_url: http://127.0.0.1:7000
#     engines: [google, bing, duckduckgo]
#   brave:                   # Brave Search API (provider `brave_api`); needs the `brave` secret
#     enabled: true
#   x:                       # search_x: X posts through xAI's x_search (design doc 80); inert without the `xai` secret or `/login xai`
#     enabled: true
#     model: grok-4.7        # any xAI model that supports agent tools
#     timeout_ms: 90000      # one Grok search can take tens of seconds

# browser:                   # embedded headless browser, browser_* tools (design doc 23)
#   enabled: true
#   allow_private_network: true    # false blocks localhost / RFC1918 / link-local again (Settings > Network)
#   import_cookies: false    # adds browser_list_profiles + browser_import_cookies:
#                            # the agent loads the user's real browser cookies and acts as them (§7)
#   profile: shared          # 209: one sealed browser profile for the web view tile and every agent browser of the
#                            # mesh (sign in once in the tile); off: no push, load or save (rows kept until cleared)

# computer:                  # see and drive this desktop, computer_* tools (design doc 27)
#   enabled: true            # the OS permission prompts are the consent; actions still ask
#   max_edge: 1568           # screenshot long edge after logical scaling (~1280 for Anthropic click accuracy)

# link:                      # drive this harness from a phone through the relay (design doc 31). `t --link` / `/link` pair; `t --install-service` runs it as a node (32).
#   enabled: false           # `/link on`, Settings › Link or hand-edit; the uplink dials out, nothing listens
#   relay: wss://relay.theuth.io   # self-host: your `theuth-relay` URL
#   name: \"\"                 # display name in the QR and on devices; \"\" → hostname
#   max_devices: 16
#   guest_handshakes_per_minute: 10  # share-link guests (design doc 59 §8.3) admitted per minute; over budget → CLOSE 3

# nodes:                     # harnesses on other machines this one can run sessions on (design doc 32). `/nodes add <key>` pairs; the node runs `t --install-service`.
#   enabled: true            # false: no dials, nodes invisible to the model and the UI; nodes.yaml kept

# schedule:                  # scheduled runs from <data_dir>/schedules/*.md files (design doc 34); `/schedules` lists them
#   enabled: true            # false: no watcher, no actor, no tool, no prompt block
#   run_timeout_secs: 1800   # a run still going after this is cancelled and reported as `timeout`

# mesh:                      # engines form a leaderless mesh over iroh; hub duties are a lease won by stability (design doc 67 §4). `t mesh status|pair|add|remove|hub`
#   relay: https://iroh.relay.theuth.io   # iroh relay when hole punching fails; self-host: your `iroh-relay` URL; `none` → bind locally, never dial a relay
#   discovery: true          # DNS/pkarr + mDNS peer lookup; false → dial remembered addresses only (one laptop, no network)
#   name: \"\"                 # display name other engines see; \"\" → hostname
#   bind: ~                  # `ip:port` the iroh endpoint binds; ~ → every interface, an OS-chosen port. Several engines on one box: 127.0.0.1:<port> each
#   hub: eligible            # prefer | eligible | never — this engine's willingness to hold the hub lease
#   probation_secs: 21600    # a newly joined engine cannot take the lease from an incumbent for this long
#   handover_secs: 600       # an incumbent keeps the lease this long after a better engine appears, then hands over
#   heartbeat_secs: 10       # lease heartbeat over gossip
#   lapse_secs: 30           # heartbeats missed for this long = the lease lapsed
#   capabilities_secs: 300   # how often harnesses / repos / display / gpu are re-advertised
#   workspaces: []           # repo roots this engine advertises for task placement
#   class: ~                 # server | desktop | laptop | container | vm — declare the machine class; ~ → detect (a sandbox VM's host writes `vm`, design doc 79)

# sandbox:                   # sandbox VMs: isolated Linux desktops on this machine that join the mesh as engines (design doc 79). `t mesh add --vm`, `t sandbox ls|start|stop|rm|desktop`
#   enabled: true            # false: no actor, no tools, no RPC; records and VMs on disk untouched (VMs keep running detached)
#   vm:
#     enabled: true          # false: docker/ssh engines only
#     runtime: microsandbox  # the only value in v1
#     home: \"\"               # MSB_HOME; \"\" = the SDK default (~/.microsandbox), shared with the user's own msb
#     image: ghcr.io/basedatum/theuth-vm   # tag = this binary's version; `--bootstrap` = `theuth-vm-base:trixie` + first-boot script
#     defaults:
#       cpus: auto           # auto = clamp(host_cores / 4, 2, 4)
#       memory_mib: auto     # auto = clamp(host_mem / 4, 2048, 8192)
#       root_disk_mib: 20480
#       max_cpus: auto       # live-resize ceiling; auto = host_cores
#       max_memory_mib: auto # auto = host_mem / 2
#       desktop: true
#       geometry: 1440x900
#       network: public_host # public_host | public_host_private | public_only | none
#     autostart: true        # start recorded VMs with this engine
#     max_vms: 4
#     resize_settle_secs: 20
#     loopback_mesh: false   # publish the VM's iroh UDP port on 127.0.0.1 and pair over it; off = relay path (a ≥ 1500 B host→guest datagram wedges the guest NIC, 79 S1 #4)
#   create_timeout_secs: 900 # sandbox_create waits this long for `ready`
#   step_timeout_secs: 600   # per bring-up step and per command inside the VM
#   health_interval_secs: 60 # status + metrics reconcile → SandboxEvent::Changed on change
#   max_concurrent_bringups: 2
#   snapshots_keep: 5        # oldest auto-snapshots pruned; named ones kept
#   handoff_timeout_secs: 1800   # desktop_handoff waits this long for Done

# roster:                    # agents as files in <data_dir>/agents/*.md, one desk each on the hub (design doc 67 §6)
#   decision_wait_secs: 120  # `escalate` waits this long for a live answer before parking the task
#   episode_idle_secs: 21600 # a desk idle this long closes its episode and writes its journal
#   episode_context_pct: 60  # … or at this percent of its context window
#   journal_max_bytes: 8192  # ceiling on roster/journal/<name>.md
#   theuth_model:            # the built-in theuth agent's model (its desk and the work it starts); unset = same as new sessions. Settings › Chat writes it

# tasks:                     # task records, wakes, checkout, disposition (design doc 67 §7); `t tasks list`
#   orphan_secs: 120         # a checkout whose engine is unreachable this long is orphaned and the owner re-woken
#   stale_secs: 900          # a `doing` task with no progress note this long is stale on the board
#   note_secs: 300           # work sessions write a progress note at least this often
#   cross_task_write_cap: 20 # tasks a hired agent's session may change outside its reporting line (theuth and the user are uncapped)
#   cross_task_write_window_secs: 86400 # the cap counts writes within this window; 0 = for the session's life
#   backoff_min_secs: 120    # no-progress back-off start, doubling …
#   backoff_max_secs: 1800   # … to this ceiling; a human comment resets it

# update:                    # self-update (design doc 39): check the release bucket, verify the signature, replace this
#                            # binary on disk; takes effect at the next start. Nothing is ever asked. `t --update` forces a check.
#   enabled: true
#   url: https://s3.rwx.dev/theuth/releases/latest.json

# artifact:                  # `show` tool: files the agent makes for the user, as cards in the chat (design doc 35); `/artifacts` lists them
#   enabled: true            # false: no `show` tool, no prompt block, no /artifacts; blobs and index kept
#   fonts: []                # extra directories searched for PDF fonts, besides system fonts and the embedded set

# app:                       # `app` tool: interactive OpenUI screens, rendered on desktop/web (design doc 46)
#   enabled: true            # false: no tool, no prompt, no parser helper, no app RPC; existing apps stay readable as source

# voice:                     # read replies aloud (design doc 43): Pocket TTS in-process, audio streamed to the frontend;
#                            # voice notes in, spoken replies out on Signal/Slack (design doc 45); live dictation (design doc 54)
#   enabled: true            # false: no actor, `speak` answers \"not running\", the desktop shows no button or mic
#   voice: peter_yearsley    # one of the checkpoint's voices; `voice_status` lists them
#   model_dir: ~             # a local Pocket TTS checkpoint directory; default <data_dir>/models/pocket-tts-en-2026-10
#   stt_model_dir: ~         # Parakeet TDT ONNX checkpoint (encoder-model.onnx, decoder_joint-model.onnx, vocab.txt); default <data_dir>/models/parakeet-tdt.
#                            # Every ONNX model (STT, dictation, diarisation, decision) shares one ONNX Runtime: the .app bundles it; otherwise
#                            # <data_dir>/models/onnxruntime/libonnxruntime.{dylib,so} (fetched by the desktop app, or `$ORT_DYLIB_PATH`, or Homebrew)
#   stt_idle_secs: 600       # unload the speech-to-text model after this long unused; 0 keeps it resident
#   dictation_model_dir: ~   # nemotron-speech-streaming-en ONNX dir (encoder.onnx, decoder_joint.onnx, tokenizer.model); default <data_dir>/models/nemotron-streaming-en
#   dictation_max_secs: 300  # a running dictation stops itself here (design doc 54)
#   player_buffer_ms: 200    # the daemon speaker's ring: how far ahead of playback synthesis may run (design doc 76)
#   player_device: ~         # output device by name for the daemon speaker; default the system output

# talk:                      # a live, interruptible spoken conversation with one session (design doc 76): STT and TTS local
#   enabled: true            # false: no actor, `talk_*` answers \"talk is disabled\"
#   barge_in_min_ms: 400     # stage 1: the gate stays open this long before playback pauses (Holding)
#   decide_ms: 900           # stage 2 fallback: a hold with no transcript yet resumes as noise this long after onset (raise if the STT lands later)
#   barge_in_vad: 0.7        # VAD probability a frame must exceed to count as speech while the agent speaks (Listening/Thinking keep 0.5)
#   end_of_turn_ms: 700      # the turn commits this long after an utterance closes without terminal punctuation
#   end_of_turn_punct_ms: 250  # the same, when the utterance ends in . ? !
#   hold: pause              # pause | none — whether stage 1 pauses the player while the barge-in classifier decides
#   backchannels: [yeah, yep, yes, mhm, mm, mm-hmm, uh-huh, ok, okay, right, sure, hm, hmm, i see, got it]  # said alone: an acknowledgement, not a cut-in
#   echo_ratio: 0.8          # a candidate that fuzzy-prefixes (Levenshtein ratio ≥ this) the agent's last 2 s of speech is echo
#   aec: prefer              # prefer | off — VoiceProcessingIO on macOS when prefer; off = plain mic, for measurements
#   turn_model: none         # none | smart-turn — end-of-turn model (refused until <data_dir>/models/smart-turn exists)

# meeting:                   # record a meeting: the meeting app's audio + the mic as Opus tracks, live briefs from the notch (design doc 53); macOS 15
#   enabled: true            # false: no actor, `meeting_*` answers \"meetings are disabled\"
#   max_minutes: 180         # hard stop; warning (NearMaxLength) at max_minutes - 10
#   context_tokens: 32000    # per-session compaction trigger for the meeting session (02 §4.1)
#   batch_secs: 25           # transcript batch cadence
#   batch_utterances: 6      # transcript batch size
#   live_model_dir: ~        # Nemotron streaming dir; default <data_dir>/models/nemotron-streaming-en (the dictation checkpoint)
#   topic_ttl_secs: 600      # how long a topic button lives unmentioned
#   max_topics: 6            # most topic buttons at once
#   models_dir: ~            # phase 4: diarization models root (nemotron-diarization/, pyannote/); default <data_dir>/models
#   live_tools: [meeting_topic, meeting_brief, brief_search, brief_put, knowledge_search, knowledge_get, meeting_read, meeting_search, search_web, read_url]  # 06: the only tools the meeting session sees while recording, all called directly
#   live_prompts: [meeting, knowledge, web]  # 06: the only prompt blocks it sees while recording
#   live_hint_limit: 2       # 06 §1.5: lines per <knowledge> note while recording; 0 = none

# models:                    # where the local models run (design doc 58 steps 12–13). Check with `t --bench-models [--accelerator X]`.
#   accelerator: auto        # auto | cpu | webgpu | directml | cuda. auto = Metal on an Apple-silicon Mac (candle); for the ONNX sessions
#                            # (STT, dictation, diarisation) the WebGPU plugin EP — Dawn on D3D12 / Vulkan / Metal, any GPU — when
#                            # <data_dir>/models/onnxruntime/plugins/{onnxruntime_providers_webgpu.dll | lib….so | lib….dylib} exists
#                            # (Windows: dxcompiler.dll + D3D12\\ beside it); CUDA for the decision model when this binary was built
#                            # with `decision/cuda`; else CPU. auto takes WebGPU only on a discrete hardware GPU (NVIDIA/AMD); an
#                            # integrated one (Intel, Apple, APU) or a software adapter (SwiftShader, llvmpipe, WARP — a GPU-less box)
#                            # logs one WARN and stays on the CPU. webgpu forces the plugin on whatever adapter exists, discrete first
#                            # (WARN + CPU when the plugin is missing; WARN + software adapter when that is all there is); directml is
#                            # explicit-only (Microsoft's DirectML ORT build ended at 1.24.4); cuda only affects candle. A GPU that
#                            # fails to initialise logs one WARN and that engine runs on the CPU.
#   webgpu_adapter: ~        # substring of the WebGPU adapter's vendor/description to use when several GPUs answer (e.g. NVIDIA, Intel)

# decision:                  # local typed-probability oracle in front of the reasoning model (design doc 57): laya in candle
#                            # answers typed questions (choice / bool / score) with a probability per option; a per-family gate
#                            # accepts or escalates to today's model turn. Every family starts in shadow (runs, logs, changes nothing).
#   enabled: true            # false: the actor answers every ask with escalate(disabled); nothing loads
#   device: auto             # auto (Metal f16 on an Apple-silicon Mac, else models.accelerator: cuda f32 or CPU f32) | cpu | metal | cuda (warn + CPU when unavailable)
#   model_dir: ~             # the `default` checkpoint dir; default <data_dir>/models/decision: model.safetensors, encoder/config.json,
#                            # tokenizer/tokenizer.json, rl_agent_config.json — the Hugging Face convaiinnovations/laya layout.
#                            # The desktop app fetches it from the release bucket at start like the speech models (54 §9,
#                            # scripts/release-models.sh); the daemon re-checks on every ask, so no restart once it lands.
#   checkpoints:             # one engine per checkpoint (57 §4.1 item 6). A checkpoint is resident only while in use (57 §2.1):
#                            # loaded by the first question (the meeting warms it at Recording), released when the meeting stops /
#                            # the browser session closes, and by idle_unload as the safety net. Nothing loads at daemon start.
#     default: ~             # = model_dir; a bare path, or { dir: <path>, idle_unload: 3m, kind: laya | gliner } (kind defaults to laya;
#                            # gliner = fastino/GLiNER2.5-Decide @ 5a7adf72 (57 §2.2): model.safetensors, tokenizer.json, encoder_config/config.json;
#                            # `gliner: { kind: gliner }` resolves <data_dir>/models/decision-gliner (scripts/fetch-models.sh fetches it); f32, ~1.9 GB resident)
#     browser: ~             # cklxx/laya-browser v10, same layout; default <data_dir>/models/decision-browser
#                            # (scripts/fetch-laya-browser.sh until release-models publishes it). A family's `checkpoint: <name>` picks one.
#                            # Its idle_unload defaults to 2m (loop steps are sub-second; a run never trips it).
#   idle_unload: 3m          # drop a checkpoint after this long unused (~0.9 GB back); checkpoints.<name>.idle_unload overrides per checkpoint
#   log_text: false          # true: store the question text in <data_dir>/decisions/*.jsonl, not only its hash
#   families:                # gate per decision family: mode shadow | active | off; accept iff top1 ≥ tau_top and top1 − top2 ≥ tau_margin;
#                            # budget (tier 1, default 400ms) and escalate_budget (tier 2 cheap-model call, default 3s) per family.
#                            # Entries patch this table (any subset of fields); a prefix key sets every family under it:
#                            # `meeting: { mode: active }` is what Settings → Labs (57 §7.1) writes — mode, train and share together.
#                            # Flip a family to active only after its shadow log shows acceptable agreement (57 §2; `decision_stats`).
#                            # train: true (57 §11, default false) captures every batch the family's oracle handled — text, mentions,
#                            # questions, tier-2 answer, outcome, tap/dismiss — to <data_dir>/decisions/training/<prefix>/<day>.jsonl,
#                            # secret-scrubbed, local only; `t --decision-export meeting` folds it into the phrasing-bench fixture shape.
#                            # share: true (57 §7.1, default false) is consent to upload those rows to theuth; needs train (train: false
#                            # clears it); recorded now, honoured by the uploader once the relay endpoint ships (step 11).
#                            # embed: true (126 §7.2, default false) returns the pooled encoder state with each decision — one
#                            # tensor copy per batch; the `double.*` families (double.approval/ask/attention/packet/draft/correction)
#                            # ship active with it on; everything else pays nothing.
#     meeting.kind:    { mode: shadow, tau_top: 0.60, tau_margin: 0.20 }  # small-talk Bool: skip at P ≥ 0.6 with no mentions (57 §3.5)
#     meeting.brief:   { mode: shadow, tau_top: 0.60, tau_margin: 0.30 }  # ≤ 4 cached briefs: publish at P ≥ 0.6, margin ≥ 0.3
#     meeting.subject: { mode: shadow, tau_top: 0.50, tau_margin: 0.00 }  # tier-2 subject extraction rows (cheap model, never the oracle)
#     browser.operation:   { mode: shadow, tau_top: 0.50, tau_margin: 0.00, checkpoint: browser, none: false, cut_ok: true, budget: 2s }
#     browser.target:      { mode: shadow, tau_top: 0.25, tau_margin: 0.00, checkpoint: browser, none: false, cut_ok: true, budget: 2s }
#     browser.type_target: { mode: shadow, tau_top: 0.25, tau_margin: 0.00, checkpoint: browser, none: false, cut_ok: true, budget: 2s }
#                            # ^ the browser System-1 loop's three jev-shaped questions (57 §4.1): `browser: { mode: active }` lets
#                            # `browser_run` act; shadow decides and hands back; off hides the tool. none/cut_ok: the fine-tune
#                            # learned a closed option list and right-cut states, so no `none` control and a cut state is not an error.
#     agent.readable: { mode: shadow, tau_top: 0.85, tau_margin: 0.40 }

# double:                    # the owner's double (design doc 126): predicts your answer to every approval / ask / packet
#                            # card before you see it, learns from what you did next, and earns per-class autonomy
#                            # (L0 observe → L1 suggest → L2 act after a grace). Settings › Double; `t --double-*`.
#   enabled: false           # restart. true: the store under <data_dir>/double/, the chip on cards, the double_* tools
#   capture: { approvals: true, asks: true, attention: true, packets: true, drafts: true }   # which cards it learns from
#   ladder:
#     grace_secs: 30         # L2 countdown before it answers (Revision 1); you can take any card over
#     caps: { read_only: 2, mutating: 1, destructive: 1 }   # max rung by risk; destructive never passes L1
#     mutating_opt_in: false # default for new classes: a mutating class needs an explicit opt-in to reach L2
#   training:
#     venue: auto            # auto | local | node:<name> | off — auto picks a non-laptop, non-VM mesh engine with a GPU
#     window: \"02:00-06:00\"  # local hours Tier B (LoRA) may run; on AC power, user idle, no meeting, nominal thermals
#     idle_min: 15
#     run_cap_min: 90        # minutes of training per night; a run resumes the next night
#     triggers: { labels: 200, days: 7 }   # Tier B eligibility, whichever first; Tier A runs every 25 labels or nightly
#     share_data: true       # ship scrubbed bundles to your own GPU nodes (never a sandbox VM)
#     tiers:
#       a: { enabled: true }
#       b: { enabled: true, local: false, rank: 8, batch: 16, epochs: 3, lr: 1.0e-4 } # local: train on this machine (idle-only, aborts on activity); off = GPU node or nothing
#       c: { enabled: false } # full fine-tune: owner action only (Settings › Rebuild)
#   retention_days: 365      # rows older than this go, except golden rows and the last 1,000 per class
#   model:
#     checkpoint: default    # a `decision.checkpoints` name; restart
#     adapter_rank: 8        # restart

# dreaming:                  # the nightly own-context routine (design doc 136): tidies knowledge, mines the day's
#                            # corrections into cases, proposes bounded skill edits behind a replay gate, journals; undo per night.
#   enabled: true            # false: no routine file, no signals.json, no prompt block; the 136 knowledge keys still apply
#   window: [\"01:00\", \"06:00\"]   # local; the sensor fires inside it when the Mac is idle
#   idle_minutes: 20
#   min_sessions: 1          # below this the night is Recall + Journal only
#   heavy_day: { concepts: 300, sessions: 40 }   # allows a second window the same night
#   budget_usd: 1            # per night; Settings › Dreaming; Dream now takes its own budget
#   run_timeout: 45m
#   models: { triage: null, tidy: null, propose: null, judge: null }   # null = the design's default per tier
#   caps: { triage: 300, merges: 40, archives: 60, verifications: 15, cases: 60, edits_per_target: 4, targets: 3, rows_per_week: 3 }
#   lifetimes: { record: 14, fact: 30, procedure: 90, entity: 60 }
#   gate: { margin: 0.1, max_regression: 0.2, min_val_cases: 3, val_fraction: 0.34 }
#   preferences: { auto_from_explicit: true, cap_lines: 20 }
#   digest: \"Mon 08:00\"
#   home_engine: null        # mesh: only this engine dreams; null = the engine that installed the routine

# autoresearch:              # measured experiment loops (design doc 140): one study = a benchmark, a check, a scope and
#                            # budgets; the harness measures, gates against the noise and keeps or reverts in its own worktree.
#   enabled: true            # false: no tools, no prompt block, no RPC beyond \"disabled\"; existing study files stay paused
#   root: null               # null = <data_dir>/autoresearch (sqlite, worktrees, custody, run logs)
#   defaults:                # what a new draft card is pre-filled with (R1-D1)
#     repeats: 3
#     bench_timeout: 600s
#     checks_timeout: 300s
#     crash_retries: 2
#     budget: { usd: 20, per_experiment_usd: 1.5, hours: 10, experiments: 120 }
#     parallel: 1
#   gate: { z: 2.0, z_fast: 3.5, z_discard: 1.0, z_suspicious: 8.0, confirm_runs: 2, remeasure_after: 6, remeasure_every: 5 }
#   noise: { max_cv: 0.05 }  # baseline sigma/mean above this refuses to start
#   plateau: { window: 12 }  # experiments without a keep: explore once, then stop
#   pause: { crash_streak: 4, custody_refusals: 2 }
#   review: { model: null, on_suspicious: revert, pause_after: 3 }   # model: null = another vendor than the executor; \"off\" disables
#   models: { executor: null, advisor: null }   # null = agent.model (R1-D5) / 125's advisor
#   effort: { executor: high, review: low, advisor: high }   # reserved: not applied yet (side calls and schedule files carry no effort)
#   brief: { max_bytes: 12288, recent: 8, dead_ends: 30, ideas: 12, playbook_bytes: 4096, code_map_tokens: 1200 }
#   novelty: { threshold: 0.6 }
#   bench: { lock: builtin, max_load: 0.5, wait: 20m }   # lock: builtin | \"<wrapper command>\" (e.g. ~/.theuth/bin/build-lock)
#   max_parallel: 4
#   confirm_budget_usd: 50   # draft cards above this need the user to type the amount
#   keep_finished_days: 14   # finished worktrees are removed after this
#   git: { author: null }    # null = theuth autoresearch <noreply@theuth.local>
#   notify: { new_best: false }   # channel pings during a study (R1-D6: off)

# a2a:                       # remote agents over A2A 1.0 (design doc 50 §6); peers/exports live in <data_dir>/a2a/peers.yaml, `/a2a reload` applies edits
#   enabled: false           # false: no HTTP listener, HTTP peers dormant (no card fetch or credential load). Contacts
#                            # (Settings › Network › Contacts, over the mesh endpoint) work either way; all need agent.communication.
#   listen: 127.0.0.1:7780   # SDK HTTP listener; loopback only, restart to change
#   public_url: null         # HTTPS base an existing reverse proxy publishes for exports. WARNING: without it the
#                            # listener is plain HTTP for loopback development only; never forward that port to a network.
#   mdns_types: [_a2a._tcp, _agent._tcp, _lad-a2a._tcp]   # browsed while an Add contact window is open; the first is advertised
#   contact_window_secs: 600 # how long an Add contact window (and its invite ticket) stays open

tools:
  policy:
    read_only: allow
    mutating: ask
    destructive: ask
  # approvals: false   # never prompt; `/approvals off always` writes this
  # pty_shell: ~        # program for `start` PTY jobs; default $SHELL, on Windows pwsh.exe else powershell.exe (58 §0.3)

# secrets:                   # design doc 37; `/secrets`, Settings › Secrets, `t --secret-set`
#   key_store: auto          # auto | keychain | file — where the vault key lives; auto = keychain when one answers, else <data_dir>/secrets.key (0600) with a warning
#   tools: true              # false: no secret_* tools, no prompt block, `{{secret:…}}` stays literal, nothing is scrubbed; the vault, /secrets and the harness's own keys are unaffected
#   request_timeout_secs: 1800   # `secret_request` gives up (the model sees \"no answer\") after this

rpc:
  listen: 127.0.0.1:7777

# signal:                   # phone frontend; runs in `t --headless`. Pair with `t --link-signal`.
#   enabled: true           # daemon starts the channel when linked
#   device_name: theuth        # shown in the phone's Linked devices list
#   owner: \"+15551234567\"   # set by --link-signal / signal_link; owner-only commands
#   allowed: []             # more E.164s; each gets its own thread

# slack:                    # Slack frontend; runs in `t --headless`. `t --slack-setup` creates the app and fills
#                           # this in; `t --slack-manifest` prints the manifest for the manual path (tokens → `t --secret-set slack_bot` / `slack_app`).
#   enabled: true           # daemon opens the socket when tokens are present
#   owner: U0123456789      # owner-only commands; may click any approval; receives unbound approvals
#   allowed: []             # more user ids; each owns their threads
#   channels: []            # channel ids where @theuth is honoured; empty = DMs only
#   tools: all              # all | errors | none — task cards per tool call
#   reasoning: true         # post the quoted reasoning message
#   approvals: all          # all | threads — approvals for sessions not bound to a thread go to the owner DM (all) or nowhere (threads)

# telegram:                 # Telegram frontend; runs in `t --headless`. `t --telegram-setup` stores the token and fills
#                           # this in (manual path: `t --secret-set telegram_bot`). The token is secret `telegram_bot`, never here.
#   enabled: true           # daemon polls when the token is present
#   owner: 123456789        # user id; set by --telegram-setup / telegram_link / the deep link; owner-only commands; receives unbound cards
#   allowed: []             # more user ids; each owns their threads
#   groups: []              # chat ids (−100…) where @mentions and commands are honoured; empty = private chats only
#   tools: all              # all | errors | none — activity cards per tool batch
#   reasoning: true         # reasoning text in the expandable quote (the thinking indicator stays)
#   approvals: all          # all | threads — where approvals/questions for non-Telegram sessions go (98 §10.1)
";

const DEFAULT_AGENTS: &str =
    "# External harnesses runnable as subagents over the Agent Client Protocol
# (spawn_subagent{agent: <name>}). `/acp` lists, probes and adds entries from the ACP registry.
# opencode:
#   command: opencode
#   args: [acp]
#   description: OpenCode, local install (1.18 or 2.x; model/effort set via session/set_config_option, design doc 06 §8.4)
#   skills: true               # default; false: no theuth skills written into the child's cwd (design doc 75 §4)
#   skills_dir: .agents/skills # default, relative to the child's cwd; .claude/skills for Claude Code
#   mcp: true                  # default; false opts out of the theuth MCP shim (list_skills/load_skill/send_message, read_file/edit/move_text/copy_text/indent/clip) in session/new (75 §9)
# codex-acp:
#   command: npx
#   args: [-y, \"@agentclientprotocol/codex-acp\"]
";

/// The bundled `harness` skill (design doc 07 §4): what the agent needs to
/// operate its own harness, loaded on demand through the skills index.
const HARNESS_SKILL: &str = include_str!("../harness.md");

/// Creates the data-dir skeleton on first run. Existing files are never
/// overwritten, except `skills/harness.md`, `skills/visualize.md`,
/// `skills/pdf.md`, `skills/loops.md`, `skills/routines.md`, `skills/apps.md`,
/// `skills/board.md`, `skills/decide.md`, `skills/double.md`, `skills/native-apps.md` and `skills/cells.md`, which the binary owns and rewrites so
/// they always describe the running version. 136: `skills/dreaming.md`,
/// `sensors/dreaming.py` (binary-owned) and `schedules/dreaming.md` (once)
/// are written by `dreaming::DreamingPlugin::start`, which reads
/// `dreaming.enabled` — this runs before the config is loaded.
fn first_run(data_dir: &Path, config_path: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(data_dir.join("mcp"))?;
    std::fs::create_dir_all(data_dir.join("acp"))?;
    let skills = data_dir.join("skills");
    std::fs::create_dir_all(&skills)?;
    for (name, body) in [
        ("harness.md", HARNESS_SKILL.to_owned()),
        ("visualize.md", surface::visualize_skill()),
        ("pdf.md", artifact::pdf::skill()),
        ("loops.md", schedule::tools::LOOPS_SKILL.to_owned()),
        ("routines.md", schedule::tools::ROUTINES_SKILL.to_owned()),
        ("apps.md", app::skill()),
        ("board.md", document::tools::BOARD_SKILL.to_owned()),
        ("decide.md", decision::tool::DECIDE_SKILL.to_owned()),
        ("double.md", double::tool::DOUBLE_SKILL.to_owned()),
        ("native-apps.md", computer::NATIVE_APPS_SKILL.to_owned()),
        ("cells.md", cells::prompt::SKILL.to_owned()), // 171:A1
    ] {
        let path = skills.join(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(body.as_str()) {
            std::fs::write(path, body)?;
        }
    }
    let agents = data_dir.join("acp").join("agents.yaml");
    if !agents.exists() {
        std::fs::write(agents, DEFAULT_AGENTS)?;
    }
    if !config_path.exists() {
        std::fs::write(config_path, DEFAULT_CONFIG)?;
    } else if let Err(e) = upgrade_communication_line(config_path) {
        tracing::warn!(
            "{}: agent.communication line not upgraded: {e}",
            config_path.display()
        );
    }
    let prompt = data_dir.join("prompt.md");
    if !prompt.exists() {
        std::fs::write(
            prompt,
            "<!-- Standing instructions for the assistant; read at every model call. -->\n",
        )?;
    }
    Ok(())
}

/// 183 §6.1: rewrite [`OLD_COMMUNICATION_LINE`] to [`COMMUNICATION_LINE`] (atomic replace);
/// `false` when the file holds no such line (already upgraded, or the user's own value).
fn upgrade_communication_line(config_path: &Path) -> std::io::Result<bool> {
    let text = std::fs::read_to_string(config_path)?;
    if !text.lines().any(|l| l == OLD_COMMUNICATION_LINE) {
        return Ok(false);
    }
    let mut out: String = text
        .lines()
        .map(|l| {
            if l == OLD_COMMUNICATION_LINE {
                COMMUNICATION_LINE
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    if text.ends_with('\n') {
        out.push('\n');
    }
    platform::files::write_atomic(config_path, out.as_bytes())?;
    Ok(true)
}

fn main() -> anyhow::Result<()> {
    platform::version::set(env!("CARGO_PKG_VERSION"));
    // Before clap, tokio, logging or anything that could touch stdout: the
    // worker's stdout is the protocol channel (08 §9).
    if std::env::args().nth(1).as_deref() == Some("subprocess") {
        std::process::exit(i32::from(worker::run()));
    }
    // 159 Z: the autoresearch handlers (begin → brief render → per-kind sections, log → gate →
    // finalize) are deep chains of large async fns; a debug build's poll frames pass tokio's
    // 2 MiB default worker stack once the seven kinds dispatch from one place (the golden
    // ledger overflowed at 2 MiB, passed at 3). Reserved, not committed: 8 MiB costs nothing
    // until used.
    let result = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_stack_size(8 << 20)
        .build()?
        .block_on(run());
    // A failed start (a plugin refused) still unlinks engine.json (63 §2.1).
    rpc::engine::release();
    result
}

/// `t double train-b` (126 Revision 2, W3): the Tier B job a node runs — the same
/// `double::train_b::TierB::train` W2's mesh `train_on` calls in-process, with `Never` as the
/// pause signal. Exit 0 with `out` populated; 2 = bad input / missing checkpoint; 3 = the run
/// cap paused it (re-run to resume); 1 = any other failure.
fn double_train_b(
    data_dir: &Path,
    decision_cfg: &decision::DecisionConfig,
    double_cfg: &double::DoubleConfig,
    bundle: &Path,
    out: &Path,
    device: &str,
) -> i32 {
    use decision::candle::lora::{Base, TRAIN_BUILD};
    use double::train_b::{Bundle, Hooks, Never, TierB, TrainConfig, TrainOutcome};

    if !TRAIN_BUILD {
        eprintln!(
            "double train-b: this `t` was built without `--features decision/train`; the \
             encoder's fused kernels cut the gradient silently, so training is refused"
        );
        return 2;
    }
    let device = match device {
        "auto" => decision_cfg.device,
        "cpu" => decision::Device::Cpu,
        "metal" => decision::Device::Metal,
        "cuda" => decision::Device::Cuda,
        other => {
            eprintln!("double train-b: --device {other}: auto | metal | cuda | cpu");
            return 2;
        }
    };
    let gpu = device.gpu();
    let dev = match gpu.open() {
        Ok(d) => d,
        Err(e) => {
            eprintln!("double train-b: {e}");
            return 2;
        }
    };
    let dir = decision_cfg.model_dir_in(data_dir);
    if !dir.is_dir() {
        eprintln!(
            "double train-b: no checkpoint at {} (fetch it with the models downloader first)",
            dir.display()
        );
        return 2;
    }
    let bundle = match Bundle::read(bundle) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("double train-b: bundle: {e:#}");
            return 2;
        }
    };
    let t = std::time::Instant::now();
    let base = match Base::open(&dir, dev, gpu.dtype()) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("double train-b: load {}: {e:#}", dir.display());
            return 1;
        }
    };
    println!(
        "loaded {} ({} tensors, {:?} {:?}) in {:.1}s; {} rows in the bundle",
        dir.display(),
        base.tensors.len(),
        base.device.location(),
        base.dtype,
        t.elapsed().as_secs_f64(),
        bundle.rows.len()
    );
    let mut cfg = TrainConfig::from_double(double_cfg);
    cfg.venue = format!("node:{}", platform::process::hostname());
    let mut on_progress = |p: &double::train_b::Progress| {
        println!(
            "step {}/{} epoch {}/{} loss {:.4} elapsed {}s",
            p.step,
            p.total_steps,
            p.epoch,
            p.epochs,
            p.loss,
            p.elapsed.as_secs()
        );
    };
    let hooks = Hooks {
        pause: &Never,
        on_progress: &mut on_progress,
    };
    match TierB::train(&bundle, &cfg, &base, out, hooks) {
        Ok(TrainOutcome::Finished(c)) => {
            println!(
                "candidate {} in {}: {} steps, train loss {:.4}, hold-out log-loss {:.4} acc {:.3} ece {:.3} (n {})",
                c.id,
                c.dir.display(),
                c.manifest.steps,
                c.manifest.metrics.train_loss,
                c.manifest.metrics.holdout_log_loss,
                c.manifest.metrics.accuracy,
                c.manifest.metrics.ece,
                c.manifest.metrics.n_holdout
            );
            0
        }
        Ok(TrainOutcome::Paused { step, checkpoint }) => {
            println!(
                "run cap reached at step {step}; checkpoint in {} — re-run to resume",
                checkpoint.display()
            );
            3
        }
        Err(e) => {
            eprintln!("double train-b: {e:#}");
            1
        }
    }
}

async fn run() -> anyhow::Result<()> {
    // reqwest links `ring`, slack/relay link `aws-lc-rs`; with both present rustls
    // panics on the first TLS dial unless a process provider is chosen (21 §2.1).
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let cli = Cli::parse();
    if cli.slack_manifest {
        print!("{}", slack::MANIFEST);
        return Ok(());
    }
    if cli.app_parser_hash {
        println!("{}", app::HASH.trim());
        return Ok(());
    }

    let data_dir = cli
        .data_dir
        .or_else(|| dirs::home_dir().map(|h| h.join(".theuth")))
        .ok_or_else(|| anyhow::anyhow!("cannot determine data dir; pass --data-dir"))?;

    // 136: the dry run reads a (copied) data dir and must write nothing — not
    // even `first_run`'s config.yaml / prompt.md.
    if let Some(Cmd::Dreaming {
        cmd: DreamingCmd::DryRun {
            data_dir: dir,
            json,
        },
    }) = &cli.cmd
    {
        let dir = dir.as_ref().unwrap_or(&data_dir);
        let report = dreaming::dry_run(dir).await?;
        if *json {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            print!("{}", dreaming::dry_run::render(&report));
        }
        return Ok(());
    }

    let config_path = cli.config.unwrap_or_else(|| data_dir.join("config.yaml"));
    first_run(&data_dir, &config_path)?;
    let config = Figment::new()
        .merge(Yaml::file(&config_path))
        .merge(Env::prefixed("THEUTH_").split("__"));
    match cli.cmd {
        Some(Cmd::Mcp { session }) => {
            return mcp::run(&data_dir, &listen(&data_dir, &config), session).await;
        }
        Some(Cmd::Double {
            cmd:
                DoubleCmd::TrainB {
                    bundle,
                    out,
                    device,
                },
        }) => {
            let models_cfg: models::accelerator::ModelsConfig =
                config.extract_inner("models").unwrap_or_default();
            models::accelerator::set(models_cfg.accelerator);
            let decision_cfg: decision::DecisionConfig =
                config.extract_inner("decision").unwrap_or_default();
            let double_cfg: double::DoubleConfig =
                config.extract_inner("double").unwrap_or_default();
            let code = tokio::task::spawn_blocking(move || {
                double_train_b(
                    &data_dir,
                    &decision_cfg,
                    &double_cfg,
                    &bundle,
                    &out,
                    &device,
                )
            })
            .await?;
            std::process::exit(code);
        }
        Some(cmd) => return cmd_67(&data_dir, &config, cmd).await,
        None => {}
    }
    // 58 step 12: one process-wide knob every model loader reads; set before any plugin starts.
    let models_cfg: models::accelerator::ModelsConfig =
        config.extract_inner("models").unwrap_or_default();
    models::accelerator::set(models_cfg.accelerator);
    models::onnx::prefer_webgpu_adapter(models_cfg.webgpu_adapter);
    if cli.bench_models {
        let _ = tracing_subscriber::fmt()
            .with_env_filter(logs::env_filter(
                &std::env::var("RUST_LOG").unwrap_or_default(),
            ))
            .with_writer(std::io::stderr)
            .try_init();
        let decision_cfg: decision::DecisionConfig =
            config.extract_inner("decision").unwrap_or_default();
        let accel = cli.accelerator.unwrap_or(models_cfg.accelerator);
        let code =
            tokio::task::spawn_blocking(move || bench_models::run(&data_dir, &decision_cfg, accel))
                .await?;
        std::process::exit(code);
    }
    if cli.update {
        let cfg: update::UpdateConfig = config.extract_inner("update").unwrap_or_default();
        return update_cli(&cfg, &data_dir, cli.stop_cells).await;
    }
    if let Some(prefix) = cli.decision_export {
        let format: decision::training::Format = cli.format.parse().map_err(anyhow::Error::msg)?;
        let store = decision::training::Store::new(data_dir.join("decisions").join("training"));
        let v = decision::training::export(&store, &prefix, cli.since, format)?;
        let n = match &v {
            serde_json::Value::Array(a) => a.len(),
            other => other["batches"].as_array().map_or(0, Vec::len),
        };
        let text = serde_json::to_string_pretty(&v)?;
        match cli.out {
            Some(path) => {
                std::fs::write(&path, text)?;
                eprintln!(
                    "wrote {n} examples from {} to {}",
                    store.family_dir(&prefix).display(),
                    path.display()
                );
            }
            None => println!("{text}"),
        }
        return Ok(());
    }
    if let Some(name) = cli.secret_set {
        let op = secret_cli::Op::Set {
            name,
            note: cli.secret_note,
        };
        return secret_cli::run(&data_dir, &config, op).await;
    }
    if let Some(name) = cli.secret_rm {
        return secret_cli::run(&data_dir, &config, secret_cli::Op::Rm(name)).await;
    }
    if cli.secret_list {
        return secret_cli::run(&data_dir, &config, secret_cli::Op::List).await;
    }
    if cli.slack_setup {
        return slack::setup::run(&data_dir, cli.app, &mut slack::setup::Terminal).await;
    }
    if cli.telegram_setup {
        return telegram::setup::run(&data_dir, &mut telegram::setup::Tty).await;
    }
    if cli.link {
        return link_cli::run(&data_dir, &config).await;
    }
    if cli.install_service {
        return service::install(
            &data_dir,
            &config_path,
            &config,
            service::Opts {
                relay: cli.relay,
                name: cli.name,
                pair: !cli.no_pair,
            },
        )
        .await;
    }
    if cli.uninstall_service {
        return service::uninstall().await;
    }

    // 63 §0.1: one engine per data dir. The lock decides; a refusal names
    // the holder. `THEUTH_LAUNCH` marks the desktop app's daemon (18 §4).
    use rpc::engine::Host;
    let host = if cli.headless {
        if std::env::var_os("THEUTH_LAUNCH").is_some() {
            Host::Desktop
        } else {
            Host::Headless
        }
    } else if cli.link_signal {
        Host::Signal
    } else if cli.acp {
        Host::Acp
    } else if cli.prompt.is_some() {
        Host::Prompt
    } else {
        Host::Tui
    };
    // Every engine serves (63 §6.4): the configured address, or — for a
    // TUI/-p/--acp without `--serve` — an ephemeral port recorded once bound.
    let listen = if cli.serve || cli.headless {
        Some(link_cli::listen(&config))
    } else {
        config
            .find_value("rpc.listen")
            .ok()
            .and_then(|v| v.into_string())
    };
    // `Some` = attached to the running engine (63 §0.3): TUI, `-p`, `--acp`.
    let attached = match rpc::engine::acquire(&data_dir, host, listen) {
        Ok(lease) => {
            rpc::engine::hold(lease);
            None
        }
        Err(rpc::engine::Error::Running(running)) => {
            let attach = matches!(host, Host::Tui | Host::Prompt | Host::Acp);
            if !attach || running.url.is_none() {
                eprintln!("{running}");
                // 146 D6: a second engine on one data dir is not a crash —
                // headless / desktop daemons exit 0 so launchd
                // (`SuccessfulExit=false`), systemd (`on-failure`) and the
                // desktop do not restart-loop against the holder.
                let code = if matches!(host, Host::Headless | Host::Desktop) {
                    0
                } else {
                    3
                };
                std::process::exit(code);
            }
            match rpc::client::connect(&running).await {
                Ok(ws) => Some((std::sync::Arc::new(ws), running)),
                Err(e) => {
                    eprintln!("{running}\ncould not attach: {e:#}");
                    std::process::exit(3);
                }
            }
        }
        Err(e) => return Err(e.into()),
    };

    // The TUI owns the terminal; logs go to a file. INFO and below through the non-blocking
    // appender; WARN and above synchronously on a second appender to the same file, so the
    // last warnings before a native crash (the WebGPU EP, 149) are on disk when the process
    // dies without draining the non-blocking worker. Both open the file O_APPEND and write
    // one line per call, so lines never interleave.
    let logs = data_dir.join("logs");
    std::fs::create_dir_all(&logs)?;
    let (writer, _guard) =
        tracing_appender::non_blocking(tracing_appender::rolling::daily(&logs, "theuth.log"));
    let writer = tracing_appender::rolling::daily(&logs, "theuth.log")
        .with_max_level(tracing::Level::WARN)
        .and(writer.with_min_level(tracing::Level::INFO));
    // ORT's INFO flood and iroh's pkarr warnings are muted in `logs::engine_filter`; a
    // kameo handler that holds its mailbox ≥ 1 s logs a WARN (`logs::SlowHandlers`, 176).
    tracing_subscriber::fmt()
        .with_env_filter(logs::engine_filter(
            &std::env::var("RUST_LOG").unwrap_or_default(),
        ))
        .with_writer(writer)
        .with_ansi(false)
        .finish()
        .with(logs::SlowHandlers)
        .init();
    // A panic's message goes to stderr, which the TUI owns in raw mode: log it too.
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        tracing::error!("panic: {info}");
        default_hook(info);
    }));

    // V8's one-time setup must happen on the main thread before any plugin
    // thread builds an isolate, and the env knobs are exported while the
    // process is still effectively single-threaded (23 §3.1). The warmup must stay
    // FIRST: its obscura isolate lives for the whole process and so fixes V8's shared
    // read-only heap to obscura's snapshot, which js-runtime's isolates (V8's built-in
    // snapshot) can share but not the other way round (see `browser::warmup`).
    let browser_cfg = config
        .extract_inner::<browser::BrowserConfig>("browser")
        .unwrap_or_default();
    browser::warmup(&browser_cfg);
    // The documents helper and the app parsers run in-process (crate js-runtime), each bundle on its own
    // thread: initialise V8 and build the first isolate here, on the main thread, also when the browser
    // (and so its warmup) is disabled. Here = synchronously in this future, which `main`'s `block_on`
    // drives on the main thread, before `Runtime::new` starts any plugin; a spawned task would race the
    // plugins. Idempotent; the isolate is built under the current tokio handle (nothing is dropped
    // inside this async context that may not be).
    js_runtime::init_platform();

    let rt = Runtime::new(config, data_dir);
    // 187: MCP servers start only on their home engine, so the tools plugin needs this
    // engine's mesh id before the mesh plugin (later in the list, absent under `--acp`) runs.
    if let Some(id) = mesh::stored_self_id(&rt.data_dir) {
        let name = rt
            .section::<mesh::config::MeshConfig>("mesh")
            .map(|c| c.name.trim().to_owned())
            .unwrap_or_default();
        let name = if name.is_empty() {
            mesh::self_name()
        } else {
            name
        };
        tools::set_machine(&id, &name);
    }
    let attached_client = attached.is_some();

    // Pairing needs no TUI, agent or tools; the plugin cancels `shutdown`
    // when the phone has scanned (12 §3).
    let plugins: Vec<Box<dyn Plugin>> = if let Some((ws, running)) = attached {
        // A client of the running engine: no engine plugins here (63 §2.3).
        if cli.acp {
            vec![Box::new(rpc_compose::AcpPlugin { remote: Some(ws), module: rpc_module })]
        } else {
            let prompt = match cli.prompt {
                Some(p) if p.is_empty() => Some(std::io::read_to_string(std::io::stdin())?),
                p => p,
            };
            // Takeover (63 §2.3): the engine this process would have run —
            // serving RPC, so the desktop app finds it again (§4.2).
            let start_engine: tui::StartEngine = Box::new(|rt| {
                Box::pin(async move { start_plugins(&rt, engine_plugins(false, false)).await })
            });
            vec![Box::new(tui::TuiPlugin {
                module: rpc_module,
                resume: cli.resume,
                prompt,
                attached: Some(tui::Attached {
                    client: ws,
                    running,
                    start_engine,
                }),
            })]
        }
    } else if cli.link_signal {
        vec![Box::new(signal::SignalPlugin {
            channel: false,
            link: Some(signal::LinkOpts {
                owner: cli.owner,
                device_name: cli.device_name,
            }),
        })]
    } else if cli.acp {
        // stdout is the protocol channel: no TUI, no line printer; tracing
        // already goes to the file (06 §8.13).
        vec![
            Box::new(tools::ToolsPlugin),
            Box::new(secrets::SecretsPlugin),
            Box::new(document::DocumentPlugin),
            Box::new(knowledge::KnowledgePlugin),
            Box::new(web::WebPlugin),
            Box::new(browser::BrowserPlugin),
            Box::new(scrape::ScrapePlugin),
            Box::new(codeindex::CodeindexPlugin),
            Box::new(routes::RoutesPlugin),
            Box::new(computer::ComputerPlugin),
            Box::new(artifact::ArtifactPlugin),
            Box::new(app::AppPlugin),
            Box::new(search_providers::SearchProvidersPlugin),
            Box::new(agent::AgentPlugin),
            // 136: the nightly routine's plugin (bundled skill/sensor/routine, signals, tools, RPC); after agent.
            Box::new(dreaming::DreamingPlugin),
            // 140: autoresearch studies (store, `Studies` actor, tools, prompt block, skill); after agent.
            Box::new(autoresearch::AutoresearchPlugin),
            // 171: user cells (`Cells` actor, kernels, run store); after agent and tools.
            Box::new(cells::CellsPlugin), // 171:E1
            // The `search` tool (49 §4.4): registration only.
            Box::new(rpc_compose::SearchToolsPlugin),
            // 63 §6.4: attachable, on a loopback port; stdout untouched.
            Box::new(rpc_compose::RpcPlugin { implicit: true, module: rpc_module }),
            Box::new(signal::SignalPlugin {
                channel: false,
                link: None,
            }),
            Box::new(slack::SlackPlugin { channel: false }),
            Box::new(telegram::TelegramPlugin { channel: false }),
            Box::new(rpc_compose::AcpPlugin { remote: None, module: rpc_module }),
        ]
    } else {
        let mut plugins = engine_plugins(cli.headless, cli.serve || cli.headless);
        if !cli.headless {
            let prompt = match cli.prompt {
                Some(p) if p.is_empty() => Some(std::io::read_to_string(std::io::stdin())?),
                p => p,
            };
            plugins.push(Box::new(tui::TuiPlugin {
                module: rpc_module,
                resume: cli.resume,
                prompt,
                attached: None,
            }));
        }
        plugins
    };

    // The pairing-only run and an attached client have no registry to
    // expect participants on.
    if cli.link_signal || attached_client {
        for plugin in plugins {
            let name = plugin.name();
            plugin
                .start(&rt)
                .await
                .map_err(|e| anyhow::anyhow!("plugin {name} failed to start: {e:#}"))?;
            tracing::info!(plugin = name, "started");
        }
    } else {
        start_plugins(&rt, plugins).await?;
        // 158 §12.2: theuth as an MCP server, iff `mcp_server.enabled`; the catalogue calls the API in-process.
        mcp_server::boot(
            &rt,
            std::sync::Arc::new(rpc::client::Local::new(rt.clone(), rpc_module)),
        )
        .await;
    }

    // 79 smoke bug 7: every normal exit names its origin (an rpc `shutdown` logs its peer in `api.rs`).
    let (origin, reason) = tokio::select! {
        _ = rt.shutdown.cancelled() => ("shutdown token cancelled (rpc shutdown, plugin or actor)", "shutdown"),
        _ = tokio::signal::ctrl_c() => ("SIGINT", "shutdown"),
        _ = sigterm() => ("SIGTERM", "shutdown"),
        _ = parent_gone() => ("parent process gone", "parent_gone"),
    };
    tracing::warn!("exiting: {origin}");
    // 109 §9.2: drain before the token fires — every turn's cancel token is
    // its child. An rpc `shutdown` drained already; this one is then a no-op.
    agent::supervisor::drain_engine(&rt, reason).await;
    rt.shutdown.cancel();
    // Give actors a moment to flush (sqlite writes, terminal restore), then
    // leave without waiting for the runtime: a `spawn_blocking` pipe drain
    // whose child never closed its end would otherwise pin the process.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    drop(_guard);
    rpc::engine::release();
    std::process::exit(0)
}

/// Resolves once the process named by `THEUTH_PARENT_PID` is gone; never when
/// it is unset. The Windows desktop app sets it on its daemon (58 §0.4): the
/// app's Job Object kills us when it exits, this is the belt to that braces
/// (a job the app failed to close, a pid_alive-visible crash). Polled, 5 s.
async fn parent_gone() {
    let Some(pid) = std::env::var("THEUTH_PARENT_PID")
        .ok()
        .and_then(|p| p.parse::<u32>().ok())
    else {
        return std::future::pending().await;
    };
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
        if !platform::process::pid_alive(pid) {
            tracing::info!(parent = pid, "desktop app gone; shutting down");
            return;
        }
    }
}

/// Resolves on SIGTERM (launchd/systemd stop, `kill`); never on Windows.
async fn sigterm() {
    #[cfg(unix)]
    if let Ok(mut term) = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
    {
        term.recv().await;
        return;
    }
    std::future::pending().await
}

fn rpc_module(server: rpc_core::Server) -> jsonrpsee::RpcModule<rpc_core::Server> {
    rpc_composition(server).0
}

/// Assemble dispatch and inert discovery from the production owner list.
fn rpc_composition(server: rpc_core::Server) -> (jsonrpsee::RpcModule<rpc_core::Server>, rpc_core::Discovery) {
    use wire::RuntimeWire as _;
    let rt = server.rt.clone();
    rpc_compose::prepare(&server.rt);
    let composition = rpc_core::merge_described([
        rpc::rpc_module(server.clone()),
        rpc_compose::rpc_module(server.clone()),
        agent::rpc_module(server.clone()),
        a2a::rpc_module(server.clone()),
        app::rpc_module(server.clone()),
        artifact::rpc_module(server.clone()),
        browser::rpc_module(server.clone()),
        document::rpc_module(server.clone()),
        knowledge::rpc_module(server.clone()),
        link::rpc_module(server.clone()),
        roster::rpc_module(server.clone()),
        search_providers::rpc_module(server.clone()),
        sessionsync::rpc_module(server.clone()),
        signal::rpc_module(server.clone()),
        slack::rpc_module(server.clone()),
        talk::rpc_module(server.clone()),
        tasks::rpc_module(server.clone()),
        telegram::rpc_module(server.clone()),
        tools::rpc_module(server.clone()),
        update::rpc_module(server.clone()),
        web::rpc_module(server.clone()),
        computer::rpc_module(server.clone()),
        mcp_server::rpc_module(server.clone()),
        codeindex::rpc_module(server.clone()),
        nodes::rpc_module(server.clone()),
        schedule::rpc_module(server.clone()),
        secrets::rpc_module(server.clone()),
        webprofile::rpc_module(server.clone()),
        meeting::rpc_module(server.clone()),
        dreaming::rpc_module(server.clone()),
        autoresearch::rpc_module(server.clone()),
        cells::rpc_module(server.clone()),
        share::rpc_module(server.clone()),
        sandbox::rpc_module(server),
    ]).expect("production RPC descriptors");
    let discovery = rpc_core::Discovery {
        descriptors: composition.descriptors,
        events: rt.wire_schema(),
        plugins: rt.extension(DeclaredPlugins::default).0.get().cloned().unwrap_or_default(),
    };
    (composition.module, discovery)
}

#[derive(Clone, Default)]
struct DeclaredPlugins(std::sync::Arc<std::sync::OnceLock<Vec<String>>>);

/// The same selected plugins declare before any start hook can open a service.
async fn declare_plugins(rt: &Runtime, plugins: &mut [Box<dyn Plugin>]) -> anyhow::Result<jsonrpsee::RpcModule<rpc_core::Server>> {
    use wire::RuntimeWire as _;
    let names = plugins.iter().map(|plugin| plugin.name().to_owned()).collect();
    anyhow::ensure!(rt.extension(DeclaredPlugins::default).0.set(names).is_ok(), "plugin declarations are frozen");
    for plugin in plugins {
        let name = plugin.name();
        plugin.declare_wire(rt).await
            .map_err(|e| anyhow::anyhow!("plugin {name} failed to declare: {e:#}"))?;
    }
    rt.freeze_wire().await?;
    let (module, discovery) = rpc_composition(rpc_core::Server::new(rt.clone()));
    rpc_core::freeze_discovery(rt, &discovery, env!("CARGO_PKG_VERSION"))?;
    Ok(module)
}

/// Export uses precisely the production plugin/owner lists, without start hooks.
#[cfg(test)]
async fn export_wire_contract(out: &Path) -> anyhow::Result<()> {
    let rt = Runtime::new(Figment::new(), std::env::temp_dir());
    let module = declare_plugins(&rt, &mut engine_plugins(true, true)).await?;
    let mut names: Vec<_> = module.method_names().collect();
    names.sort_unstable();
    std::fs::create_dir_all(out)?;
    let cached = rpc_core::discovery_document(&rt, rpc_core::DiscoveryView::Mesh)?;
    let document: serde_json::Value = serde_json::from_str(cached.document.0.get())?;
    std::fs::write(out.join("openrpc.json"), serde_json::to_string_pretty(&document)? + "\n")?;
    std::fs::write(out.join("methods.json"), serde_json::to_string_pretty(&names)?)?;
    std::fs::write(out.join("surface-catalog.json"), serde_json::to_string_pretty(&surface::catalog_schema())?)?;
    std::fs::write(out.join("surface-catalog-compact.txt"), surface::catalog_compact())?;
    let auxiliary = serde_json::json!({
        "agent": agent::rpc_forward::client_error_schema(),
        "surface": surface::client_schema(),
    });
    std::fs::write(out.join("client-auxiliary.json"), serde_json::to_string_pretty(&auxiliary)?)?;
    Ok(())
}

#[cfg(test)]
mod wire_contract_tests {
    use super::*;
    use std::collections::BTreeSet;

    /// Generation entry point used by `just ts` and `just wire-schema`.
    #[tokio::test]
    async fn export_bindings() {
        let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../bindings");
        export_wire_contract(&out).await.unwrap();
    }

    #[tokio::test]
    async fn real_composition_descriptors_match_served_methods() {
        let rt = Runtime::new(Figment::new(), std::env::temp_dir());
        declare_plugins(&rt, &mut engine_plugins(true, true)).await.unwrap();
        let (module, discovery) = rpc_composition(rpc_core::Server::new(rt));
        discovery.descriptors.validate_methods(module.method_names()).unwrap();
        // 194's source-scan uniqueness check belongs at the real composition:
        // validate_methods rejects duplicate names, including aliases and unsubscribers.
        for (name, owner) in [
            ("title_effort", "agent"),
            ("set_title_effort", "agent"),
            ("polish_effort", "agent"),
            ("set_polish_effort", "agent"),
            ("vision_effort", "agent"),
            ("set_vision_effort", "agent"),
            ("escalate_effort", "agent"),
            ("set_escalate_effort", "agent"),
            ("subagent_effort", "agent"),
            ("set_subagent_effort", "agent"),
            ("advisor_on", "agent"),
            ("set_advisor_default_model", "agent"),
            ("set_session_compactor_effort", "agent"),
            ("theuth_agent_effort", "roster"),
            ("set_theuth_agent_effort", "roster"),
            ("autoresearch.settings.set", "autoresearch"),
        ] {
            let method = discovery.descriptors.methods.iter()
                .find(|method| method.name == name)
                .unwrap_or_else(|| panic!("missing RPC method: {name}"));
            assert_eq!(method.owner, owner, "{name}");
        }
        let document = discovery.document().unwrap();
        let described: BTreeSet<_> = document["methods"].as_array().unwrap().iter()
            .map(|method| method["name"].as_str().unwrap()).collect();
        let served: BTreeSet<_> = module.method_names().collect();
        assert_eq!(described, served);
        assert!(described.contains("new_session"));
        assert!(document["x-theuth"]["events"].as_array().unwrap().iter()
            .any(|event| event["topic"] == "agent.events"));
    }
}

/// The engine's plugin list (every mode but `--acp` and `--link-signal`):
/// channels run in `--headless`; the RPC server always (63 §6.4) — on the
/// configured address with `serve`, else implicitly (ephemeral fallback).
fn engine_plugins(headless: bool, serve: bool) -> Vec<Box<dyn Plugin>> {
    let mut plugins: Vec<Box<dyn Plugin>> = vec![
        Box::new(tools::ToolsPlugin),
        Box::new(secrets::SecretsPlugin),
        Box::new(document::DocumentPlugin),
        Box::new(knowledge::KnowledgePlugin),
        Box::new(web::WebPlugin),
        Box::new(browser::BrowserPlugin),
        Box::new(scrape::ScrapePlugin),
        Box::new(codeindex::CodeindexPlugin),
        Box::new(routes::RoutesPlugin),
        Box::new(computer::ComputerPlugin),
        Box::new(artifact::ArtifactPlugin),
        Box::new(app::AppPlugin),
        Box::new(search_providers::SearchProvidersPlugin),
        Box::new(agent::AgentPlugin),
        // 136: the nightly routine's plugin (bundled skill/sensor/routine, signals, tools, RPC); after agent.
        Box::new(dreaming::DreamingPlugin),
        // 140: autoresearch studies (store, `Studies` actor, tools, prompt block, skill); after agent.
        Box::new(autoresearch::AutoresearchPlugin),
        // 171: user cells (`Cells` actor, kernels, run store); after agent and tools.
        Box::new(cells::CellsPlugin), // 171:E1
        Box::new(rpc_compose::SearchToolsPlugin),
        Box::new(signal::SignalPlugin {
            channel: headless,
            link: None,
        }),
        Box::new(slack::SlackPlugin { channel: headless }),
        Box::new(telegram::TelegramPlugin { channel: headless }),
        Box::new(link::LinkPlugin),
        Box::new(nodes::NodesPlugin),
        Box::new(schedule::SchedulePlugin),
        // 67: mesh → replica → roster → tasks, in that order (contract doc).
        Box::new(mesh::MeshPlugin),
        Box::new(replica::ReplicaPlugin),
        // 68: ships the agent store's session log over `theuth/sessions/1`; needs agent + replica.
        Box::new(sessionsync::SessionSyncPlugin),
        Box::new(roster::RosterPlugin),
        // 209: the shared browser profile; opens its store once the vault answers and replicates
        // `web_sites` once the replica runs (both awaited in the background).
        Box::new(webprofile::WebProfilePlugin),
        Box::new(tasks::TasksPlugin),
        Box::new(sandbox::SandboxPlugin),
        // 210: public web sharing; after mesh (identity, endpoint, vault) and replica (shares table).
        Box::new(share::SharePlugin::engine()),
        // Always registers its actor (183 W-26: contacts need it; `a2a.enabled`
        // gates the HTTP listener) and finishes in the background once `agent`
        // and `secrets` answer, so its place in this list carries no ordering
        // promise (50 §11 row 3).
        Box::new(a2a::A2aPlugin),
        Box::new(update::UpdatePlugin),
        Box::new(voice::VoicePlugin),
        Box::new(talk::TalkPlugin),
        Box::new(decision::DecisionPlugin),
        // 126: the owner's double; off unless `double.enabled: true` (a client of the bus, after decision).
        Box::new(double::DoublePlugin),
    ];
    // 58 §6.1: capture backends per OS; `MeetingStatus.supported` says what this host can do.
    plugins.push(Box::new(meeting::MeetingPlugin));
    plugins.push(Box::new(rpc_compose::RpcPlugin { implicit: !serve, module: rpc_module }));
    plugins
}

/// Start `plugins` on `rt` in order. First, 51 §8.2: the owners the registry
/// must hear from before a private root may open — every lifecycle
/// participant whose plugin runs here (each plugin honours its own
/// `<section>.enabled`, default on). Jobs is registry-internal.
async fn start_plugins(rt: &Runtime, mut plugins: Vec<Box<dyn Plugin>>) -> anyhow::Result<()> {
    declare_plugins(rt, &mut plugins).await?;
    let owners: Vec<&'static str> = [
        "artifact", "app", "browser", "computer", "voice", "talk", "routes",
    ]
    .into_iter()
    .filter(|name| {
        rt.config
            .extract_inner::<bool>(&format!("{name}.enabled"))
            .unwrap_or(true)
    })
    .collect();
    let registry_rt = rt.clone();
    tokio::spawn(async move {
        if let Some(reg) = registry_rt
            .lookup_when::<tools::ToolRegistry>(tools::REGISTRY_ACTOR)
            .await
        {
            let _ = reg.tell(tools::ExpectParticipants(owners)).await;
        }
    });
    // 147 §6.4: the CodeMap generator's side calls reach the agent actor
    // through the typed registry; only this crate knows both ends (the
    // shape of `meeting::oracle::side_caller`). Looked up per call, so the
    // order plugins start in does not matter.
    codeindex::map::generator::install_side_caller(std::sync::Arc::new(|call| {
        Box::pin(async move {
            let agents =
                runtime::kameo::actor::ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
                    .ok()
                    .flatten()
                    .ok_or_else(|| "agent is not running".to_owned())?;
            agents.ask(call).await.map_err(|e| e.to_string())
        })
    }));
    // §10.2: `codemap_generate` refuses a session the supervisor would refuse
    // before writing a job row.
    codeindex::map::generator::install_side_probe(std::sync::Arc::new(|session| {
        Box::pin(async move {
            let agents =
                runtime::kameo::actor::ActorRef::<agent::Agents>::lookup(agent::AGENT_ACTOR)
                    .ok()
                    .flatten()
                    .ok_or_else(|| "agent is not running".to_owned())?;
            agents
                .ask(agent::SideCallable { session })
                .await
                .map_err(|e| e.to_string())
        })
    }));
    for plugin in plugins {
        let name = plugin.name();
        plugin
            .start(rt)
            .await
            .map_err(|e| anyhow::anyhow!("plugin {name} failed to start: {e:#}"))?;
        tracing::info!(plugin = name, "started");
    }
    // 182: a table still unattached now has no plugin here — its parked changes no
    // longer hold the first sync. Here, so the TUI takeover's `start_engine` gets it too.
    replica::plugins_started();
    Ok(())
}

/// `t --update` (39 §3.2): one foreground check, one line on stdout.
/// Logs go to stderr so `RUST_LOG=debug t --update` shows the debug lines.
async fn update_cli(
    cfg: &update::UpdateConfig,
    data_dir: &Path,
    stop_cells: bool,
) -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .try_init();
    match cells_before_update(data_dir, stop_cells).await {
        Ok(true) => {}
        Ok(false) => {
            eprintln!("update cancelled");
            return Ok(());
        }
        Err(e) => {
            eprintln!("update: {e:#}");
            std::process::exit(1);
        }
    }
    match update::run_once(cfg, data_dir).await {
        Ok(update::Outcome::UpToDate) => println!("up to date ({})", env!("CARGO_PKG_VERSION")),
        Ok(update::Outcome::Applied { from, to }) => println!("theuth {from} → {to}"),
        Ok(update::Outcome::Available { version, path }) => println!(
            "theuth {version} available; {} is not writable — run `sudo t --update`",
            path.display()
        ),
        Err(e) => {
            eprintln!("update: {e:#}");
            std::process::exit(1);
        }
    }
    Ok(())
}

/// 171 §5.3 (F6 a, L7): the engine's jobs die with it (109 R1), so user cells running on
/// this data dir's engine are the user's call before an update: listed, then `wait` (until
/// none runs), `stop` (Stop, Kill 5 s later) or `cancel` on a TTY; without one the update
/// refuses unless `--stop-cells`. `Ok(false)` = cancelled. No engine, an unreachable one
/// or one without cells asks nothing.
async fn cells_before_update(data_dir: &Path, stop_cells: bool) -> anyhow::Result<bool> {
    use cells::rpc::KernelsApiClient;
    use std::io::IsTerminal;
    let Some(running) = rpc::engine::discover(data_dir) else {
        return Ok(true);
    };
    let Ok(ws) = rpc::client::connect(&running).await else {
        return Ok(true);
    };
    let live = match ws.cell_live().await {
        Ok(live) if !live.is_empty() => live,
        Ok(_) => return Ok(true),
        Err(e) => {
            tracing::debug!("cell_live: {e}");
            return Ok(true);
        }
    };
    let now = chrono::Utc::now().timestamp_millis();
    eprintln!("{} user cell run(s) in progress on the engine:", live.len());
    for r in &live {
        let secs = (now - r.started_ms.unwrap_or(r.queued_ms)).max(0) / 1000;
        let (m, s) = (secs / 60, secs % 60);
        eprintln!("  {} ({m}:{s:02}) · {} · {}", r.id.0, r.interpreter, r.cwd);
    }
    let tty = std::io::stdin().is_terminal();
    let mut stoppable = true;
    loop {
        let choice = if stop_cells && stoppable {
            "stop".to_owned()
        } else if !tty {
            anyhow::bail!(
                "user cells are running; wait for them, or rerun with --stop-cells to stop them"
            );
        } else {
            eprint!(
                "{} ",
                if stoppable {
                    "wait / stop / cancel?"
                } else {
                    "wait / cancel?"
                }
            );
            let mut line = String::new();
            std::io::stdin().read_line(&mut line)?;
            line.trim().to_ascii_lowercase()
        };
        match choice.as_str() {
            "wait" | "w" => {
                eprintln!("waiting for the runs to end…");
                while !ws.cell_live().await?.is_empty() {
                    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                }
                return Ok(true);
            }
            "stop" | "s" if stoppable => {
                let mut refused = None;
                for r in &live {
                    if let Err(e) = ws.cell_stop(r.id.clone(), false).await {
                        refused = Some(e);
                        break;
                    }
                }
                if let Some(e) = refused {
                    // The press guard may refuse this CLI's peer: only waiting is left.
                    eprintln!("could not stop the runs here: {e}");
                    stoppable = false;
                    continue;
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                for r in ws.cell_live().await? {
                    let _ = ws.cell_stop(r.id, true).await;
                }
                return Ok(true);
            }
            "cancel" | "c" | "" => return Ok(false),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 183 §6.1: first runs write the line commented out; an upgrade rewrites only the old
    /// template line, once, and leaves every edited value and every other line alone.
    #[test]
    fn the_template_upgrade_comments_out_only_the_untouched_line() {
        assert!(DEFAULT_CONFIG.lines().any(|l| l == COMMUNICATION_LINE));
        assert!(!DEFAULT_CONFIG.lines().any(|l| l == OLD_COMMUNICATION_LINE));
        let dir = std::env::temp_dir().join(format!("rs183-upgrade-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.yaml");
        let old = format!("agent:\n  model: m\n{OLD_COMMUNICATION_LINE}\n# tail\n");
        std::fs::write(&path, &old).unwrap();
        assert!(upgrade_communication_line(&path).unwrap());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            format!("agent:\n  model: m\n{COMMUNICATION_LINE}\n# tail\n")
        );
        assert!(!upgrade_communication_line(&path).unwrap(), "once");
        for mine in [
            "agent:\n  communication: false\n",
            "agent:\n  communication: true       # model-to-model communication (design doc 50): spawn_subagent, discover_peers, send_message.\n",
        ] {
            std::fs::write(&path, mine).unwrap();
            assert!(!upgrade_communication_line(&path).unwrap());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), mine);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
