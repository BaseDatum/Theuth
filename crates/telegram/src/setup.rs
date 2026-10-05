//! NEEDS(COMMANDS): `t --telegram-setup` (design doc 98 §10.4). Stage 1 lands
//! the terminal trait, [`Tty`] and [`deep_link`]; [`run`] is not implemented.

use std::io::Write;
use std::path::Path;

/// The interactive terminal, behind a trait for tests.
pub trait Terminal {
    /// Asks `q`, returns the trimmed line typed.
    fn prompt(&mut self, q: &str) -> anyhow::Result<String>;
    /// Asks `q` for the bot token; echoes (it is pasted, not typed — slack's rule).
    fn secret(&mut self, q: &str) -> anyhow::Result<String>;
    /// Prints a line.
    fn say(&mut self, s: &str);
}

/// Stdin / stdout.
pub struct Tty;

impl Terminal for Tty {
    fn prompt(&mut self, q: &str) -> anyhow::Result<String> {
        print!("{q}");
        std::io::stdout().flush()?;
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line)? == 0 {
            anyhow::bail!("stdin closed");
        }
        Ok(line.trim().to_owned())
    }

    fn secret(&mut self, q: &str) -> anyhow::Result<String> {
        self.prompt(q)
    }

    fn say(&mut self, s: &str) {
        println!("{s}");
    }
}

/// `t --telegram-setup`: token → vault, command scopes, pairing, config.
// NEEDS(COMMANDS): the setup flow. Stage 1 refuses.
pub async fn run(_data_dir: &Path, _term: &mut dyn Terminal) -> anyhow::Result<()> {
    Err(anyhow::anyhow!("telegram setup: not implemented yet"))
}

/// `https://t.me/<username>?start=<code>` (98 §10.4).
#[must_use]
pub fn deep_link(username: &str, code: &str) -> String {
    format!("https://t.me/{}?start={code}", username.trim_start_matches('@'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deep_link_strips_the_at() {
        assert_eq!(deep_link("@theuth_bot", "abc"), "https://t.me/theuth_bot?start=abc");
        assert_eq!(deep_link("theuth_bot", "abc"), "https://t.me/theuth_bot?start=abc");
    }
}
