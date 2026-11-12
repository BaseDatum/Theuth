use super::*;

/// Core provider flows, backed by the mounted composer.
#[derive(Debug)]
pub enum Prompt {
    /// Asking for the gateway name.
    ProviderName,
    /// Asking for the base URL.
    ProviderUrl {
        /// Name already collected.
        name: String,
    },
    /// Asking for the API key.
    ProviderKey {
        /// Name already collected.
        name: String,
        /// Normalized URL already collected.
        url: String,
    },
}

impl Prompt {
    /// Line shown above the input.
    pub fn hint(&self) -> String {
        match self {
            Prompt::ProviderName => "provider add  name  (esc cancel)".into(),
            Prompt::ProviderUrl { name } => {
                format!("provider add  {name}  base url  (esc cancel)")
            }
            Prompt::ProviderKey { name, .. } => {
                format!("provider add  {name}  api key  (esc cancel)")
            }
        }
    }

    /// Notice when Esc drops the flow.
    fn cancelled(&self) -> &'static str {
        match self {
            Prompt::ProviderName | Prompt::ProviderUrl { .. } | Prompt::ProviderKey { .. } => {
                "provider add cancelled"
            }
        }
    }

    /// The input box hides the provider's key.
    pub fn masked(&self) -> bool {
        matches!(
            self,
            Prompt::ProviderKey { .. }
        )
    }
}

impl Model {

    /// Advance a prompt flow one field. An API key is never written to the transcript.
    pub(super) fn prompt_submit(&mut self, line: &str, tx: &UnboundedSender<Command>) {
        match self.prompt.take() {
            Some(Prompt::ProviderName) => {
                self.prompt = Some(Prompt::ProviderUrl {
                    name: line.to_owned(),
                });
                self.entries.push(Entry::Info(format!("name: {line}")));
            }
            Some(Prompt::ProviderUrl { name }) => {
                self.entries.push(Entry::Info(format!("url: {line}")));
                // 177 E10: a local server needs no key.
                if agent::local::private_url(line) {
                    send(
                        tx,
                        Command::AddProvider {
                            name,
                            base_url: line.to_owned(),
                            key: String::new(),
                        },
                    );
                } else {
                    self.prompt = Some(Prompt::ProviderKey {
                        name: name.clone(),
                        url: line.to_owned(),
                    });
                }
            }
            Some(Prompt::ProviderKey { name, url }) => {
                self.entries.push(Entry::Info("api key: set".into()));
                send(
                    tx,
                    Command::AddProvider {
                        name,
                        base_url: url,
                        key: line.to_owned(),
                    },
                );
            }
            None => {}
        }
    }

    /// Drop an in-progress prompt flow.
    pub fn cancel_prompt(&mut self) {
        if let Some(p) = self.prompt.take() {
            self.entries.push(Entry::Info(p.cancelled().into()));
        }
    }
}
