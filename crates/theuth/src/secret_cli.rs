//! `t --secret-set NAME [--secret-note …]`, `--secret-rm NAME`,
//! `--secret-list` (design doc 37 §3.9): the vault for boxes without a TUI.
//! One-shot like `--slack-setup`: opens `<data_dir>/secrets.sqlite` as a
//! library; a running daemon sees the change within a second (67 §5.4).
//! Nothing is printed but the receipt; the value never reaches stdout.

use std::io::IsTerminal;
use std::path::Path;

use anyhow::bail;
use figment::Figment;

/// Which `--secret-*` flag was given.
pub enum Op {
    /// `--secret-set NAME`: a new version of `name`; `note: None` keeps the old note.
    Set {
        /// `[A-Za-z0-9_.-]{1,64}`; the vault validates.
        name: String,
        /// `--secret-note`.
        note: Option<String>,
    },
    /// `--secret-rm NAME`: the name and every version.
    Rm(String),
    /// `--secret-list`: `key: <store>` then `NAME · vN (k kept) · note · updated` per secret.
    List,
}

/// Open the vault with the `secrets:` section of `config` (defaults when
/// absent) and run `op`, printing its receipt. Vault errors become the exit
/// message.
pub async fn run(data_dir: &Path, config: &Figment, op: Op) -> anyhow::Result<()> {
    let cfg: secrets::SecretsConfig = if config.contains("secrets") {
        config.extract_inner("secrets")?
    } else {
        secrets::SecretsConfig::default()
    };
    let vault = secrets::Vault::open(data_dir, cfg.key_store).await?;
    match op {
        Op::Set { name, note } => {
            let value = read_value(&name)?;
            let info = vault
                .set(&name, &value, note.as_deref(), "cli")
                .await
                .map_err(anyhow::Error::msg)?;
            println!("stored {name} v{}", info.current);
        }
        Op::Rm(name) => {
            vault.delete(&name).await.map_err(anyhow::Error::msg)?;
            println!("removed {name}");
        }
        Op::List => {
            println!("key: {}", vault.key_store());
            for s in vault.list() {
                println!(
                    "{} · v{} ({} kept) · {} · {}",
                    s.name,
                    s.current,
                    s.versions.len(),
                    s.note,
                    s.updated
                );
            }
        }
    }
    Ok(())
}

/// The value: typed twice without echo on a tty, else the first line of stdin
/// (trailing newline dropped). Empty is refused.
fn read_value(name: &str) -> anyhow::Result<String> {
    let stdin = std::io::stdin();
    let value = if stdin.is_terminal() {
        let first = rpassword::prompt_password(format!("value for {name}: "))?;
        let again = rpassword::prompt_password("again: ")?;
        if first != again {
            bail!("values differ");
        }
        first
    } else {
        let mut line = String::new();
        stdin.read_line(&mut line)?;
        line.truncate(line.trim_end_matches(['\r', '\n']).len());
        line
    };
    if value.is_empty() {
        bail!("empty value");
    }
    Ok(value)
}
