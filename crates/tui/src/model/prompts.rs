use super::*;

/// Core provider and secret-vault flows, backed by the mounted composer.
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
    /// `/secrets add`: asking for the name.
    SecretName,
    /// `/secrets add NAME` / `r`: asking for the value (masked).
    SecretValue {
        /// Name already collected.
        name: String,
        /// `Some(v)` when the name exists: the value adds `v + 1`.
        current: Option<u32>,
    },
    /// `/secrets add`: asking for the note; an empty line keeps the old one (or none).
    SecretNote {
        /// Name already collected.
        name: String,
        /// Value already collected. Never written to the transcript.
        value: String,
    },
    /// `/secrets` `d`: typing the name again deletes it, anything else keeps it.
    SecretDelete {
        /// The name.
        name: String,
    },
    /// `/peers` `a`: typing the answer to one exact pending request (50 §5.3).
    PeerAnswer {
        /// The session the human acts for.
        session: SessionId,
        /// The request.
        reply_to: Uuid,
        /// Its authenticated sender (the hint).
        from: String,
        /// `session` sent the request and is blocked on it: the answer
        /// resolves its own wait instead of travelling to a peer.
        resolve: bool,
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
            Prompt::SecretName => "secrets add  name  (esc cancel)".into(),
            Prompt::SecretValue {
                name,
                current: None,
            } => format!("secrets add  {name}  value  (esc cancel)"),
            Prompt::SecretValue {
                name,
                current: Some(v),
            } => format!(
                "secrets add  {name}  value  (replaces v{v} → v{}; esc cancel)",
                v + 1
            ),
            Prompt::SecretNote { name, .. } => {
                format!("secrets add  {name}  note  (⏎ none; esc cancel)")
            }
            Prompt::SecretDelete { name } => {
                format!("secrets  delete {name}?  type the name to confirm  (esc cancel)")
            }
            Prompt::PeerAnswer {
                reply_to,
                from,
                resolve,
                ..
            } => format!(
                "peers  {} {} · {}  answer  (esc cancel)",
                if *resolve {
                    "resolve own wait on"
                } else {
                    "answer for this session to"
                },
                short_uuid(*reply_to),
                from
            ),
        }
    }

    /// Notice when Esc drops the flow.
    fn cancelled(&self) -> &'static str {
        match self {
            Prompt::ProviderName | Prompt::ProviderUrl { .. } | Prompt::ProviderKey { .. } => {
                "provider add cancelled"
            }
            Prompt::SecretName
            | Prompt::SecretValue { .. }
            | Prompt::SecretNote { .. }
            | Prompt::SecretDelete { .. } => "secrets: cancelled",
            Prompt::PeerAnswer { .. } => "peers: answer cancelled",
        }
    }

    /// The input box hides what is typed: a secret's value, a provider's key.
    pub fn masked(&self) -> bool {
        matches!(
            self,
            Prompt::ProviderKey { .. } | Prompt::SecretValue { .. }
        )
    }
}

impl Model {

    /// Advance a prompt flow one field. An API key or a secret's value is
    /// never written to the transcript.
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
            Some(Prompt::SecretName) => {
                if secrets::vault::valid_name(line) {
                    self.prompt = Some(Prompt::SecretValue {
                        name: line.to_owned(),
                        current: self.secret_current(line),
                    });
                } else {
                    self.prompt = Some(Prompt::SecretName);
                    self.entries.push(Entry::Error(bad_secret_name(line)));
                }
            }
            Some(Prompt::SecretValue { name, .. }) => {
                self.prompt = Some(Prompt::SecretNote {
                    name,
                    value: line.to_owned(),
                });
            }
            Some(Prompt::SecretNote { name, value }) => send(
                tx,
                Command::SetSecret {
                    name,
                    value,
                    note: (!line.is_empty()).then(|| line.to_owned()),
                },
            ),
            Some(Prompt::SecretDelete { name }) => {
                if line == name {
                    send(tx, Command::DeleteSecret(name));
                } else {
                    self.entries
                        .push(Entry::Info(format!("secrets: kept {name}")));
                }
            }
            Some(Prompt::PeerAnswer {
                session, reply_to, ..
            }) => send(
                tx,
                Command::AnswerPeerRequest {
                    session,
                    reply_to,
                    text: line.to_owned(),
                },
            ),
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
