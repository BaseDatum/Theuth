//! Owner: B1 — `roster::attention::Source` for install / widen / code-changed cards and `notify`
//! items (contract §10.1). Cards never time out; ids carry a `:<seq>` suffix.

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex, Weak};

use roster::attention::{AttentionItem, AttentionKind, RegisterSource, Source};

use crate::types::{CardKind, InstallCard};

/// `notify` items kept per plugin; older ones fall off.
const NOTES_PER_PLUGIN: usize = 20;

/// One `notify` item.
#[derive(Clone, Debug)]
pub(crate) struct Note {
    /// `plugin-notify:<name>:<n>`.
    pub id: String,
    /// The plugin's title (already redacted).
    pub title: String,
    /// The body (already redacted).
    pub body: Option<String>,
    /// When it arrived.
    pub at_ms: u64,
}

/// What the inbox shows for plugins: one pending card per plugin, recent notes.
#[derive(Default)]
pub(crate) struct Inbox {
    cards: BTreeMap<String, InstallCard>,
    notes: BTreeMap<String, VecDeque<Note>>,
    next_note: u64,
}

/// Shared between the actor (writer) and the registered Source (reader).
pub(crate) type SharedInbox = Arc<Mutex<Inbox>>;

impl Inbox {
    /// File (or replace) the plugin's pending card.
    pub fn file(&mut self, card: InstallCard) {
        self.cards.insert(card.plugin.clone(), card);
    }

    /// Withdraw the plugin's card (approved, uninstalled, or no longer needed).
    pub fn withdraw(&mut self, plugin: &str) {
        self.cards.remove(plugin);
    }

    /// Add a `notify` item.
    pub fn note(
        &mut self,
        plugin: &str,
        title: String,
        body: Option<String>,
        at_ms: u64,
    ) -> String {
        self.next_note += 1;
        let id = format!("plugin-notify:{plugin}:{}", self.next_note);
        let q = self.notes.entry(plugin.to_owned()).or_default();
        q.push_back(Note {
            id: id.clone(),
            title,
            body,
            at_ms,
        });
        while q.len() > NOTES_PER_PLUGIN {
            q.pop_front();
        }
        id
    }

    /// Drop everything of one plugin (uninstall).
    pub fn forget(&mut self, plugin: &str) {
        self.cards.remove(plugin);
        self.notes.remove(plugin);
    }

    fn items(&self) -> Vec<AttentionItem> {
        let mut out: Vec<AttentionItem> = self.cards.values().map(card_item).collect();
        for (plugin, notes) in &self.notes {
            out.extend(notes.iter().map(|n| AttentionItem {
                waiting_since_ms: None,
                ..AttentionItem::new(
                    n.id.clone(),
                    AttentionKind::Plugin,
                    format!("{} · plugin {plugin}", n.title),
                    n.body.clone(),
                    n.at_ms,
                )
            }));
        }
        out
    }
}

/// The attention item for a card (contract §10.1 table).
pub(crate) fn card_item(card: &InstallCard) -> AttentionItem {
    let title = match card.kind {
        CardKind::Install => format!("Install plugin {} {}?", card.plugin, card.version),
        CardKind::Widen => format!("Plugin {} asks for more", card.plugin),
        CardKind::CodeChanged => format!("Plugin {} changed its code", card.plugin),
    };
    let mut parts: Vec<String> = card.lines.iter().take(2).map(|l| l.text.clone()).collect();
    if card.kind == CardKind::CodeChanged && parts.is_empty() {
        parts.push(format!("changed: {}", card.changed_paths.join(", ")));
    }
    parts.push("open theuth to review".to_owned());
    AttentionItem {
        waiting_since_ms: None,
        expires_ms: None,
        ..AttentionItem::new(
            card.id.clone(),
            AttentionKind::Plugin,
            title,
            Some(parts.join(" · ")),
            card.at_ms,
        )
    }
}

struct PluginItems(Weak<Mutex<Inbox>>);

#[async_trait::async_trait]
impl Source for PluginItems {
    async fn items(&self) -> Vec<AttentionItem> {
        let Some(inbox) = self.0.upgrade() else {
            return Vec::new();
        };
        inbox.lock().map(|i| i.items()).unwrap_or_default()
    }
}

/// Register the Source with the attention actor once it answers (never blocks boot).
pub(crate) async fn register(rt: runtime::Runtime, inbox: SharedInbox) {
    let Some(attention) = rt
        .lookup_when::<roster::attention::Attention>(roster::attention::ATTENTION_ACTOR)
        .await
    else {
        return;
    };
    let source: Arc<dyn Source> = Arc::new(PluginItems(Arc::downgrade(&inbox)));
    if let Err(e) = attention.tell(RegisterSource(source)).await {
        tracing::warn!(error = %e, "plugins: attention source not registered");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{GrantKey, GrantLine};

    fn card(kind: CardKind) -> InstallCard {
        InstallCard {
            id: "plugin:turns:6b1f0000:00000000:0".into(),
            plugin: "turns".into(),
            version: "0.2.0".into(),
            kind,
            manifest_sha256: "6b1f".into(),
            code_sha256: None,
            risk: tools::Risk::Destructive,
            lines: vec![
                GrantLine {
                    key: GrantKey::Run,
                    text: "Runs `python3 main.py`".into(),
                    risk: tools::Risk::Destructive,
                    new: false,
                },
                GrantLine {
                    key: GrantKey::Subscribe,
                    text: "Reads agent.events: TurnFinished".into(),
                    risk: tools::Risk::ReadOnly,
                    new: false,
                },
                GrantLine {
                    key: GrantKey::Emit,
                    text: "Publishes `turns.limit_reached`".into(),
                    risk: tools::Risk::ReadOnly,
                    new: false,
                },
            ],
            changed_paths: vec![],
            unverified: false,
            dir: "/tmp/turns".into(),
            at_ms: 7,
        }
    }

    #[test]
    fn card_item_never_times_out_and_names_two_lines() {
        let item = card_item(&card(CardKind::Install));
        assert_eq!(item.kind, AttentionKind::Plugin);
        assert_eq!(item.title, "Install plugin turns 0.2.0?");
        assert_eq!(item.waiting_since_ms, None);
        assert_eq!(item.expires_ms, None);
        assert_eq!(
            item.detail.as_deref(),
            Some(
                "Runs `python3 main.py` · Reads agent.events: TurnFinished · open theuth to review"
            )
        );
    }

    #[tokio::test]
    async fn source_lists_cards_and_notes() {
        let inbox: SharedInbox = Default::default();
        inbox.lock().unwrap().file(card(CardKind::Widen));
        let id = inbox
            .lock()
            .unwrap()
            .note("turns", "Limit".into(), Some("50".into()), 9);
        assert!(id.starts_with("plugin-notify:turns:"));
        let src = PluginItems(Arc::downgrade(&inbox));
        let items = src.items().await;
        assert_eq!(items.len(), 2);
        inbox.lock().unwrap().withdraw("turns");
        assert_eq!(src.items().await.len(), 1);
    }
}
