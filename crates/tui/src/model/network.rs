use super::*;

/// The residual mesh-device pairing countdown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PairingCard {
    pub(crate) id: uuid::Uuid,
    /// The pairing string (rendered as the QR and on one line).
    key: String,
    /// Unix ms.
    expires_at_ms: u64,
    /// Whole seconds the card last showed; redraw when it changes.
    left: u64,
}

impl PairingCard {
    /// A card over `p`, showing its whole seconds left as of now.
    pub(super) fn new(p: link::Pairing) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            left: p.expires_at_ms.saturating_sub(now_ms()) / 1000,
            key: p.key,
            expires_at_ms: p.expires_at_ms,
        }
    }

    /// Seconds until expiry, floored at zero.
    fn remaining(&self) -> u64 {
        self.expires_at_ms.saturating_sub(now_ms()) / 1000
    }

    /// Card body: the QR in a fence, the key, the countdown.
    pub(super) fn body(&self) -> String {
        let left = self.left;
        format!(
            "```\n{}\n```\n{}\nexpires in {}:{:02}",
            link::qr(&self.key),
            self.key,
            left / 60,
            left % 60
        )
    }
}

/// Keep the residual mesh countdown in the same replacement group as node invites.
pub(super) fn tick_card(entries: &mut [Entry], slot: &mut Option<PairingCard>) {
    let Some(card) = slot else { return };
    let Some(Entry::Card { kind: CardKind::Component { key, .. }, text, .. }) =
        entries.iter_mut().rev().find(|entry| matches!(entry,
            Entry::Card { kind: CardKind::Component { group: "node-invite", .. }, .. }))
        else { *slot = None; return };
    if *key != card.id { *slot = None; return; }
    let left = card.remaining();
    if left == card.left { return; }
    card.left = left;
    **text = markdown::Text::complete(&if left == 0 { "pairing expired".into() } else { card.body() });
    if left == 0 { *slot = None; }
}



impl Model {



    /// `/mesh devices [list|add [name]|rm <id>|name <id> <name…>]` (69 §7), the CLI's `t mesh devices`.
    pub(super) fn mesh_cmd(&mut self, arg: &str, tx: &UnboundedSender<Command>) {
        const USAGE: &str = "usage: /mesh devices [list|add [name]|rm <id>|name <id> <name>]";
        let (sub, rest) = arg
            .split_once(char::is_whitespace)
            .map_or((arg, ""), |(s, r)| (s, r.trim()));
        if sub != "devices" {
            self.entries.push(Entry::Error(USAGE.into()));
            return;
        }
        let (verb, rest) = rest
            .split_once(char::is_whitespace)
            .map_or((rest, ""), |(s, r)| (s, r.trim()));
        match (verb, rest) {
            ("" | "list", _) => send(tx, Command::MeshDevicesList),
            ("add", name) => send(
                tx,
                Command::MeshDevicesAdd((!name.is_empty()).then(|| name.to_owned())),
            ),
            ("rm", id) if !id.is_empty() => send(tx, Command::MeshDevicesRevoke(id.to_owned())),
            ("name", rest) if rest.contains(char::is_whitespace) => {
                let (id, name) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                send(
                    tx,
                    Command::MeshDevicesRename(id.to_owned(), name.trim().to_owned()),
                );
            }
            _ => self.entries.push(Entry::Error(USAGE.into())),
        }
    }


}




/// One `/mesh devices` row: `<id>  <name>  <platform>  paired 2d ago · seen 3h ago | never seen | revoked`.
pub fn mesh_device_line(d: &link::DeviceInfo, now: u64) -> String {
    let since = |ms: u64| ago(chrono::Duration::milliseconds(now.saturating_sub(ms) as i64));
    let state = if d.revoked {
        "revoked".to_owned()
    } else if d.last_seen_ms == 0 {
        "never seen".to_owned()
    } else {
        format!("seen {}", since(d.last_seen_ms))
    };
    format!(
        "{}  {}  {}  paired {} · {state}",
        d.id,
        d.name,
        d.platform,
        since(d.paired_ms)
    )
}
