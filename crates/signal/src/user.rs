//! `message_user` outbound (design doc 24 §6): the channel as an
//! [`agent::UserSink`]. Text goes through the 09 §8 plain-text chunker with
//! picture fences as PNGs in place (164 D9), then one attachment message per
//! file with its file name.

use agent::{Outbound, UserSink};
use artifact::snapshot::{self, Piece};

use crate::channel::Handle;
use crate::text;

#[async_trait::async_trait]
impl UserSink for Handle {
    /// `"signal"`: the `via` value and the word in the tool result (24 §3).
    fn name(&self) -> &'static str {
        "signal"
    }

    /// Bound pass: the first session of `m.sessions` with a thread
    /// (`threads.recipient_for`, 09 §6). Fallback: `signal.owner`, text
    /// prefixed with `m.origin` on its own line (24 §2, §6). Not gated by the
    /// hub lease (67 decision 22): a session running here reaches its user.
    async fn deliver(&self, m: &Outbound, fallback: bool) -> Result<Option<String>, String> {
        let chan = &self.chan;
        let (to, text) = if fallback {
            if chan.owner.is_empty() {
                return Err("signal: no owner configured".into());
            }
            if !chan.transport.is_registered().await.unwrap_or(false) {
                return Err("signal: not linked".into());
            }
            let to = chan.lock().threads.recipient(&chan.owner);
            (to, format!("{}\n{}", m.origin, m.text))
        } else {
            let to = {
                let s = chan.lock();
                m.sessions
                    .iter()
                    .find_map(|id| s.threads.recipient_for(*id))
            };
            let Some(to) = to else {
                return Ok(None);
            };
            (to, m.text.clone())
        };
        for piece in snapshot::paint(&chan.rt, &text, snapshot::PHONE_WIDTH, chan.pictures).await {
            let ok = match piece {
                Piece::Text(t) => {
                    let plain = text::markdown_to_plain(&t);
                    plain.is_empty() || chan.send_chunks(&to, &plain).await.is_some()
                }
                Piece::Picture(p) => {
                    chan.send_attachment(
                        &to,
                        &p.png,
                        "image/png",
                        Some(&p.name),
                        p.title.as_deref(),
                    )
                    .await
                }
            };
            if !ok {
                return Err("signal: send failed".into());
            }
        }
        for f in &m.files {
            let bytes = tokio::fs::read(&f.path)
                .await
                .map_err(|e| format!("cannot read {}: {e}", f.path.display()))?;
            if !chan
                .send_attachment(&to, &bytes, &f.mime, Some(&f.name), None)
                .await
            {
                return Err(format!("signal: attachment {} failed", f.name));
            }
        }
        Ok(Some(format!("signal: {}", to.number)))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agent::OutboundFile;
    use runtime::{Runtime, SessionId};

    use super::*;
    use crate::SignalConfig;
    use crate::channel;
    use crate::mock::{MockTransport, SentItem};
    use crate::transport::Recipient;

    const OWNER: &str = "+15550001111";
    const FRIEND: &str = "+15550002222";

    fn fixture() -> (tempfile::TempDir, MockTransport, Handle) {
        let dir = tempfile::tempdir().unwrap();
        let rt = Runtime::new(figment::Figment::new(), dir.path().to_path_buf());
        let transport = MockTransport::new();
        let cfg = SignalConfig {
            owner: OWNER.into(),
            ..SignalConfig::default()
        };
        let handle = channel::unstarted(Arc::new(transport.clone()), cfg, rt);
        (dir, transport, handle)
    }

    fn outbound(sessions: Vec<SessionId>, text: &str, files: Vec<OutboundFile>) -> Outbound {
        Outbound {
            sessions,
            origin: "session deadbeef · untitled".into(),
            text: text.into(),
            files,
        }
    }

    #[tokio::test]
    async fn bound_session_gets_text_then_one_attachment_per_file() {
        let (dir, transport, handle) = fixture();
        let (root, child) = (SessionId::new(), SessionId::new());
        handle
            .chan
            .lock()
            .threads
            .bind(&Recipient::new(FRIEND), root);
        let path = dir.path().join("report.pdf");
        std::fs::write(&path, b"%PDF").unwrap();
        let m = outbound(
            vec![child, root],
            "**done**",
            vec![OutboundFile {
                path,
                name: "report.pdf".into(),
                mime: "application/pdf".into(),
                bytes: 4,
            }],
        );
        let r = handle.deliver(&m, false).await;
        assert_eq!(r, Ok(Some(format!("signal: {FRIEND}"))));
        let sent = transport.sent();
        assert_eq!(sent.len(), 2);
        assert!(
            matches!(&sent[0], SentItem::Text { to, text, .. } if to == FRIEND && text == "done")
        );
        assert_eq!(
            sent[1],
            SentItem::Attachment {
                to: FRIEND.into(),
                mime: "application/pdf".into(),
                bytes: b"%PDF".to_vec(),
                file_name: Some("report.pdf".into()),
                caption: None,
            }
        );
    }

    #[tokio::test]
    async fn fallback_goes_to_owner_with_origin_line() {
        let (_dir, transport, handle) = fixture();
        let m = outbound(vec![SessionId::new()], "hi", Vec::new());
        assert_eq!(handle.deliver(&m, false).await, Ok(None));
        assert!(transport.sent().is_empty());
        let r = handle.deliver(&m, true).await;
        assert_eq!(r, Ok(Some(format!("signal: {OWNER}"))));
        let sent = transport.sent();
        assert_eq!(sent.len(), 1);
        assert!(matches!(&sent[0], SentItem::Text { to, text, .. }
            if to == OWNER && text == "session deadbeef · untitled\nhi"));
    }

    #[tokio::test]
    async fn missing_file_is_an_error_after_text() {
        let (dir, transport, handle) = fixture();
        let m = outbound(
            Vec::new(),
            "see file",
            vec![OutboundFile {
                path: dir.path().join("nope.bin"),
                name: "nope.bin".into(),
                mime: "application/octet-stream".into(),
                bytes: 0,
            }],
        );
        let r = handle.deliver(&m, true).await;
        assert!(
            r.as_ref().is_err_and(|e| e.starts_with("cannot read ")),
            "{r:?}"
        );
        assert_eq!(transport.sent().len(), 1);
    }

    /// A client that draws every requested fence as `png` (164 D3).
    #[derive(kameo::Actor)]
    struct FakeRenderer(Vec<u8>);

    impl kameo::message::Message<snapshot::FenceSnapshotRequested> for FakeRenderer {
        type Reply = ();
        async fn handle(
            &mut self,
            m: snapshot::FenceSnapshotRequested,
            _: &mut kameo::message::Context<Self, ()>,
        ) {
            snapshot::answer(
                m.id,
                snapshot::FenceSnapshotAnswer::done(
                    &self.0,
                    1120,
                    600,
                    artifact::preview::Client::Desktop,
                ),
            );
        }
    }

    /// 164 D9: a raw picture fence in `message_user` text (a schedule or
    /// autoresearch delivery) goes out as a PNG in its place, then the files.
    #[tokio::test]
    async fn raw_fence_is_delivered_as_a_picture_in_place() {
        let (dir, transport, handle) = fixture();
        let (tx, rx) = tokio::sync::broadcast::channel::<Arc<serde_json::Value>>(4);
        artifact::preview::watch_feed(tx);
        std::mem::forget(rx);
        let mut image = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13];
        image.extend(b"IHDR");
        image.extend(1120u32.to_be_bytes());
        image.extend(600u32.to_be_bytes());
        image.extend([8, 6, 0, 0, 0]);
        let actor = kameo::actor::Spawn::spawn_with_mailbox(
            FakeRenderer(image.clone()),
            runtime::subscriber_mailbox(),
        );
        handle
            .chan
            .rt
            .subscribe::<FakeRenderer, snapshot::FenceSnapshotRequested>(&actor)
            .await;
        let root = SessionId::new();
        handle
            .chan
            .lock()
            .threads
            .bind(&Recipient::new(FRIEND), root);
        let path = dir.path().join("report.pdf");
        std::fs::write(&path, b"%PDF").unwrap();
        let m = outbound(
            vec![root],
            "Nightly:\n\n```mermaid\ngraph LR\n  A --> B\n```\n\n**done**",
            vec![OutboundFile {
                path,
                name: "report.pdf".into(),
                mime: "application/pdf".into(),
                bytes: 4,
            }],
        );
        assert_eq!(
            handle.deliver(&m, false).await,
            Ok(Some(format!("signal: {FRIEND}")))
        );
        let sent = transport.sent();
        assert_eq!(sent.len(), 4, "{sent:?}");
        assert!(matches!(&sent[0], SentItem::Text { text, .. } if text == "Nightly:"));
        assert_eq!(
            sent[1],
            SentItem::Attachment {
                to: FRIEND.into(),
                mime: "image/png".into(),
                bytes: image,
                file_name: Some("diagram.png".into()),
                caption: None,
            }
        );
        assert!(matches!(&sent[2], SentItem::Text { text, .. } if text == "done"));
        assert!(matches!(&sent[3], SentItem::Attachment { file_name, .. }
            if file_name.as_deref() == Some("report.pdf")));
    }
}
