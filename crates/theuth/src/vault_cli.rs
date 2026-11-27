//! `t vault …` (design doc 225): the sealed vault from a terminal. Every subcommand but `init`
//! is one RPC call on the running engine (the `cmd_67` pattern); `--engine ID` targets a mesh
//! peer, forwarded by the engine. `methods` falls back to the keys file when no engine
//! answers; `init` provisions a fresh data dir offline. Passwords come from the terminal
//! without echo, or with `--password-stdin` one per line; never from argv, never printed.

use std::io::BufRead;
use std::path::Path;

use anyhow::bail;
use jsonrpsee::core::ClientError;
use jsonrpsee::core::client::ClientT;
use jsonrpsee::rpc_params;
use jsonrpsee::ws_client::WsClient;
use serde_json::Value;

/// The typed sealed error (`VAULT_SEALED`): its message, exit 2.
const VAULT_SEALED: i32 = -32041;

/// `--engine`: the target engine.
#[derive(clap::Args, Debug)]
pub(crate) struct Target {
    /// A mesh peer's engine id (see `t mesh status`); default: this engine.
    #[arg(long, value_name = "ID")]
    engine: Option<String>,
}

/// `--password-stdin`.
#[derive(clap::Args, Debug)]
pub(crate) struct Stdin {
    /// Read the password(s) from stdin, one per line, instead of prompting.
    #[arg(long)]
    password_stdin: bool,
}

/// `t vault …` (225).
#[derive(clap::Subcommand, Debug)]
pub(crate) enum VaultCmd {
    /// Sealed or not, by what, the unlock methods and any warning.
    Status {
        #[command(flatten)]
        target: Target,
        /// The engine's `VaultStatus` as JSON.
        #[arg(long)]
        json: bool,
    },
    /// Lock secret reads now (needs a password or device method); keys leave memory at restart.
    Seal {
        #[command(flatten)]
        target: Target,
    },
    /// Unseal with the master password.
    Unseal {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        stdin: Stdin,
    },
    /// The unlock methods (read from disk when the engine is not running).
    Methods {
        #[command(flatten)]
        target: Target,
    },
    /// Add a master password (typed twice; at least 12 characters).
    AddPassword {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        stdin: Stdin,
    },
    /// Change the master password (stdin: the current one, then the new one).
    ChangePassword {
        #[command(flatten)]
        target: Target,
        #[command(flatten)]
        stdin: Stdin,
    },
    /// Remove an unlock method; the last one is refused.
    RemoveMethod {
        /// `password`, `keychain`, `file` or a device's engine id (see `t vault methods`).
        id: String,
        #[command(flatten)]
        target: Target,
    },
    /// Fresh data dir only, engine stopped: create the vault key under a master password.
    Init {
        #[command(flatten)]
        stdin: Stdin,
    },
}

/// Run one `t vault` subcommand.
pub(crate) async fn run(
    data_dir: &Path,
    config: &figment::Figment,
    cmd: VaultCmd,
) -> anyhow::Result<()> {
    match cmd {
        VaultCmd::Init { stdin } => return init(data_dir, stdin.password_stdin).await,
        VaultCmd::Methods { target } => {
            let Some(client) = try_engine(data_dir, config).await else {
                if target.engine.is_some() {
                    bail!(
                        "no harness on {}; --engine needs the running engine",
                        crate::listen(data_dir, config)
                    );
                }
                let methods = secrets::envelope::methods_offline(data_dir)?;
                println!(
                    "engine not running; methods on disk in {}:",
                    data_dir.display()
                );
                print!("{}", render_methods(&serde_json::to_value(methods)?));
                return Ok(());
            };
            let status = call(&client, "vault_status", rpc_params![target.engine]).await?;
            print!("{}", render_methods(&status["methods"]));
            return Ok(());
        }
        _ => {}
    }
    let client = engine(data_dir, config).await;
    let (status, engine) = match cmd {
        VaultCmd::Status { target, json } => {
            let status = call(&client, "vault_status", rpc_params![&target.engine]).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&status)?);
                return Ok(());
            }
            (status, target.engine)
        }
        VaultCmd::Seal { target } => (
            call(&client, "vault_seal", rpc_params![&target.engine]).await?,
            target.engine,
        ),
        VaultCmd::Unseal { target, stdin } => {
            let password = if stdin.password_stdin {
                stdin_line(&mut std::io::stdin().lock())?
            } else {
                prompt("master password: ")?
            };
            (
                call(
                    &client,
                    "vault_unseal",
                    rpc_params![&target.engine, password],
                )
                .await?,
                target.engine,
            )
        }
        VaultCmd::AddPassword { target, stdin } => {
            let password = if stdin.password_stdin {
                checked(stdin_line(&mut std::io::stdin().lock())?)?
            } else {
                new_password()?
            };
            (
                call(
                    &client,
                    "vault_add_password",
                    rpc_params![&target.engine, password],
                )
                .await?,
                target.engine,
            )
        }
        VaultCmd::ChangePassword { target, stdin } => {
            let (old, new) = if stdin.password_stdin {
                let mut lock = std::io::stdin().lock();
                (stdin_line(&mut lock)?, checked(stdin_line(&mut lock)?)?)
            } else {
                (prompt("current master password: ")?, new_password()?)
            };
            (
                call(
                    &client,
                    "vault_change_password",
                    rpc_params![&target.engine, old, new],
                )
                .await?,
                target.engine,
            )
        }
        VaultCmd::RemoveMethod { id, target } => (
            call(
                &client,
                "vault_remove_method",
                rpc_params![&target.engine, id],
            )
            .await?,
            target.engine,
        ),
        VaultCmd::Init { .. } | VaultCmd::Methods { .. } => unreachable!("handled above"),
    };
    print!("{}", render_status(&status, engine.as_deref()));
    Ok(())
}

/// `t vault init`: refuses while an engine runs on `data_dir` (it owns the keys file).
async fn init(data_dir: &Path, password_stdin: bool) -> anyhow::Result<()> {
    if rpc::engine::discover(data_dir).is_some() {
        bail!(
            "the engine is running on {}; use `t vault add-password` instead",
            data_dir.display()
        );
    }
    let password = if password_stdin {
        checked(stdin_line(&mut std::io::stdin().lock())?)?
    } else {
        new_password()?
    };
    let dir = data_dir.to_owned();
    tokio::task::spawn_blocking(move || secrets::envelope::init_password(&dir, &password))
        .await??;
    println!(
        "vault: created in {}, sealed by a master password",
        data_dir.display()
    );
    Ok(())
}

/// The running engine, else the `t --headless` hint and exit 1.
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

/// One call; a server error is its message on stderr, exit 2 when sealed, else 1.
async fn call(
    client: &WsClient,
    method: &str,
    params: jsonrpsee::core::params::ArrayParams,
) -> anyhow::Result<Value> {
    match client.request(method, params).await {
        Ok(v) => Ok(v),
        Err(ClientError::Call(e)) => {
            eprintln!("{}", e.message());
            std::process::exit(if e.code() == VAULT_SEALED { 2 } else { 1 });
        }
        Err(e) => Err(e.into()),
    }
}

/// One password from the terminal, no echo.
fn prompt(label: &str) -> anyhow::Result<String> {
    let password = rpassword::prompt_password(label)?;
    if password.is_empty() {
        bail!("empty password");
    }
    Ok(password)
}

/// A new password from the terminal: typed twice, both must match, long enough.
fn new_password() -> anyhow::Result<String> {
    let first = rpassword::prompt_password("new master password: ")?;
    let again = rpassword::prompt_password("again: ")?;
    confirm(first, &again)
}

/// `first` when `again` matches and it is long enough.
fn confirm(first: String, again: &str) -> anyhow::Result<String> {
    if first != again {
        bail!("passwords differ");
    }
    checked(first)
}

/// `password` when it has at least `MIN_PASSWORD` characters.
fn checked(password: String) -> anyhow::Result<String> {
    if password.chars().count() < secrets::envelope::MIN_PASSWORD {
        bail!(
            "a master password needs at least {} characters",
            secrets::envelope::MIN_PASSWORD
        );
    }
    Ok(password)
}

/// One line of `input`, its line ending dropped; EOF or an empty line is an error.
fn stdin_line(input: &mut impl BufRead) -> anyhow::Result<String> {
    let mut line = String::new();
    input.read_line(&mut line)?;
    line.truncate(line.trim_end_matches(['\r', '\n']).len());
    if line.is_empty() {
        bail!("no password on stdin");
    }
    Ok(line)
}

/// A `VaultStatus` on one screen: the state line, the methods, the warning.
fn render_status(s: &Value, engine: Option<&str>) -> String {
    let unseal = match engine {
        Some(id) => format!("`t vault unseal --engine {id}`"),
        None => "`t vault unseal`".to_owned(),
    };
    let who = s["name"]
        .as_str()
        .map(|n| format!(" ({n})"))
        .unwrap_or_default();
    let mut out = match s["state"].as_str().unwrap_or("unknown") {
        "unsealed" => match s["unsealed_by"].as_str() {
            Some(by) => format!("vault{who}: unsealed by {by}\n"),
            None => format!("vault{who}: unsealed\n"),
        },
        "sealed" => {
            let asking: Vec<String> = s["asking"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .map(|id| device_name(s, id))
                .collect();
            let mut line = format!("vault{who}: sealed");
            if !asking.is_empty() {
                line += &format!(" — asking {}", asking.join(", "));
            }
            if s["password_set"].as_bool() == Some(true) {
                line += &format!("; unlock with {unseal}");
            }
            line + "\n"
        }
        state => format!("vault{who}: {state}\n"),
    };
    if let Some(e) = s["error"].as_str() {
        out += &format!("reason: {e}\n");
    }
    out += &render_methods(&s["methods"]);
    if let Some(w) = s["warning"].as_str() {
        out += &format!("warning: {w}\n");
    }
    out
}

/// A device's name from its method label (`Unlock from sky-mac`), else its id.
fn device_name(s: &Value, id: &str) -> String {
    s["methods"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|m| m["id"].as_str() == Some(id))
        .and_then(|m| m["label"].as_str())
        .map_or(id, |l| l.strip_prefix("Unlock from ").unwrap_or(l))
        .to_owned()
}

/// `methods:` then `  <id>  <label>` per method (the id is what `remove-method` takes).
fn render_methods(methods: &Value) -> String {
    let list = methods.as_array().map(Vec::as_slice).unwrap_or_default();
    if list.is_empty() {
        return "methods: none\n".to_owned();
    }
    let mut out = "methods:\n".to_owned();
    for m in list {
        let id = m["id"].as_str().unwrap_or("?");
        let label = m["label"].as_str().unwrap_or("");
        out += &format!("  {id:<10} {label}\n");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn stdin_line_strips_line_endings_and_refuses_empty() {
        let mut input = std::io::Cursor::new("correct horse battery\r\nsecond line\n");
        assert_eq!(stdin_line(&mut input).unwrap(), "correct horse battery");
        assert_eq!(stdin_line(&mut input).unwrap(), "second line");
        assert!(stdin_line(&mut input).is_err());
        assert!(stdin_line(&mut std::io::Cursor::new("\n")).is_err());
    }

    #[test]
    fn confirm_refuses_mismatch_and_short() {
        assert!(confirm("correct horse battery".into(), "correct horse batterz").is_err());
        assert!(confirm("short".into(), "short").is_err());
        assert_eq!(
            confirm("twelve chars".into(), "twelve chars").unwrap(),
            "twelve chars"
        );
    }

    #[test]
    fn sealed_status_names_the_asked_device() {
        let s = json!({
            "engine": "e1", "name": "sky-cloud", "state": "sealed", "unsealed_by": null,
            "methods": [
                {"kind": "password", "id": "password", "label": "Master password", "created_ms": 1},
                {"kind": "device", "id": "abc123", "label": "Unlock from sky-mac", "created_ms": 2}
            ],
            "password_set": true, "warning": null, "asking": ["abc123"], "error": null
        });
        let out = render_status(&s, None);
        assert!(out.starts_with(
            "vault (sky-cloud): sealed — asking sky-mac; unlock with `t vault unseal`\n"
        ));
        assert!(out.contains("  abc123     Unlock from sky-mac\n"));
        assert!(!out.contains("warning:"));
    }

    #[test]
    fn warning_line_is_prefixed() {
        let s = json!({
            "state": "unsealed", "unsealed_by": "file", "methods": [],
            "warning": "vault key stored unprotected on disk — add a master password"
        });
        assert_eq!(
            render_status(&s, None),
            "vault: unsealed by file\nmethods: none\nwarning: vault key stored unprotected on disk — add a master password\n"
        );
    }
}
