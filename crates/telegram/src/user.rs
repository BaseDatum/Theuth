//! `message_user` on Telegram (design docs 24 §5, 98 §8.10): the [`UserSink`]
//! the plugin registers with the agent crate's `"user"` actor. Bound sessions
//! get a message in their thread; the owner's chat is the fallback.

use agent::{Outbound, UserSink};
use artifact::snapshot::{PHONE_WIDTH, Piece, paint};

use crate::api::{ApiError, OutFile, Target};
use crate::channel::Handle;

/// Telegram's plain `sendMessage` cap.
const TEXT_MAX: usize = 4096;

#[async_trait::async_trait]
impl UserSink for Handle {
    /// `"telegram"` (24 §3).
    fn name(&self) -> &'static str {
        "telegram"
    }

    /// Bound pass: the first session of `m.sessions` with a thread (24 §2
    /// step 1); `Ok(None)` when none has one. Fallback: the owner's chat with
    /// `_<origin>_` as the first line (24 §2 step 2). The text is painted
    /// (164 D6, D7): text pieces as `send_rich`, plain chunks when Telegram
    /// rejects the markdown (98 §8.1), pictures as photos in place; then one
    /// `send_file` per file.
    async fn deliver(&self, m: &Outbound, fallback: bool) -> Result<Option<String>, String> {
        let chan = &self.chan;
        let (to, text, label) = if fallback {
            let owner = chan
                .owner()
                .ok_or_else(|| "telegram: no owner paired".to_owned())?;
            let to = Target {
                chat: i64::try_from(owner).map_err(|e| format!("telegram: owner id: {e}"))?,
                thread: None,
            };
            (
                to,
                format!("_{}_\n{}", m.origin, m.text),
                "owner".to_owned(),
            )
        } else {
            let Some(to) = m.sessions.iter().find_map(|s| chan.target_for(*s)) else {
                return Ok(None);
            };
            (to, m.text.clone(), chan.chat_label(to.chat))
        };
        if !text.trim().is_empty() {
            for piece in paint(&chan.rt, &text, PHONE_WIDTH, chan.cfg.pictures).await {
                match piece {
                    Piece::Text(text) => match chan.send(&to, &text, None, None, None).await {
                        Ok(_) => {}
                        Err(ApiError::Api { code: 400, .. }) => {
                            for chunk in markdown::chunks(&markdown::plain(&text), TEXT_MAX) {
                                chan.api
                                    .send_text(&to, &chunk, None)
                                    .await
                                    .map_err(|e| format!("telegram: {e}"))?;
                            }
                        }
                        Err(e) => return Err(format!("telegram: {e}")),
                    },
                    Piece::Picture(p) => {
                        let file = OutFile {
                            name: p.name,
                            mime: "image/png".to_owned(),
                            bytes: p.png,
                        };
                        chan.send_file(&to, &file, p.title.as_deref(), None)
                            .await
                            .map_err(|e| format!("telegram: {e}"))?;
                    }
                }
            }
        }
        for f in &m.files {
            let bytes = tokio::fs::read(&f.path)
                .await
                .map_err(|e| format!("cannot read {}: {e}", f.path.display()))?;
            let file = OutFile {
                name: f.name.clone(),
                mime: f.mime.clone(),
                bytes,
            };
            chan.send_file(&to, &file, None, None)
                .await
                .map_err(|e| format!("telegram: {e}"))?;
        }
        Ok(Some(format!("telegram: {label}")))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use agent::OutboundFile;
    use artifact::snapshot;
    use runtime::SessionId;

    use super::*;
    use crate::api::mock::{Call, png};
    use crate::channel::tests::{OWNER, fixture, fixture_with};
    use crate::{TelegramConfig, threads};

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

    fn outbound(sessions: Vec<SessionId>, text: &str, files: Vec<OutboundFile>) -> Outbound {
        Outbound {
            sessions,
            origin: "session deadbeef · untitled".into(),
            text: text.into(),
            files,
        }
    }

    /// 164 D9 (g): a raw picture fence in `message_user` text (a schedule or
    /// autoresearch delivery) goes out as a photo in its place, then the files.
    #[tokio::test]
    async fn raw_fence_is_delivered_as_a_photo_in_place() {
        let f = fixture(false);
        artifact::preview::watch_feed(Arc::new(|| 1));
        let image = png(1120, 600);
        let actor = kameo::actor::Spawn::spawn_with_mailbox(
            FakeRenderer(image.clone()),
            runtime::subscriber_mailbox(),
        );
        f.rt.subscribe::<FakeRenderer, snapshot::FenceSnapshotRequested>(&actor)
            .await;
        let chat = i64::try_from(OWNER).unwrap();
        let session = f.bind(&threads::key(chat, None), OWNER);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("report.pdf");
        std::fs::write(&path, b"%PDF").unwrap();
        let m = outbound(
            vec![session],
            "Nightly:\n\n```mermaid\ngraph LR\n  A --> B\n```\n\n**done**",
            vec![OutboundFile {
                path,
                name: "report.pdf".into(),
                mime: "application/pdf".into(),
                bytes: 4,
            }],
        );
        let handle = Handle {
            chan: Arc::clone(&f.chan),
        };
        let label = handle.deliver(&m, false).await.unwrap().unwrap();
        assert!(label.starts_with("telegram: "), "{label}");
        let calls = f.api.calls();
        assert_eq!(calls.len(), 4, "{calls:?}");
        assert!(matches!(&calls[0], Call::SendRich { md, .. } if md == "Nightly:"));
        assert!(matches!(
            &calls[1],
            Call::SendFile { name, mime, bytes, caption: None, .. }
                if name == "diagram.png" && mime == "image/png" && *bytes == image
        ));
        assert!(matches!(&calls[2], Call::SendRich { md, .. } if md == "**done**"));
        assert!(matches!(&calls[3], Call::SendFile { name, .. } if name == "report.pdf"));
        assert!(
            !format!("{calls:?}").contains("graph LR"),
            "no fence body leaves the channel"
        );
    }

    /// `pictures: false` (164 D10): the fence is its D7 text in the owner
    /// fallback message, no file, no fence body.
    #[tokio::test]
    async fn pictures_off_delivers_fallback_text() {
        let f = fixture_with(
            TelegramConfig {
                pictures: false,
                ..TelegramConfig::default()
            },
            false,
        );
        let handle = Handle {
            chan: Arc::clone(&f.chan),
        };
        let m = outbound(
            vec![SessionId::new()],
            "Nightly:\n\n```mermaid\ngraph LR\n  A --> B\n```\n\n**done**",
            Vec::new(),
        );
        assert_eq!(
            handle.deliver(&m, true).await,
            Ok(Some("telegram: owner".into()))
        );
        let calls = f.api.calls();
        assert_eq!(calls.len(), 1, "{calls:?}");
        let Call::SendRich { md, to, .. } = &calls[0] else {
            panic!("{calls:?}");
        };
        assert_eq!(to.chat, i64::try_from(OWNER).unwrap());
        assert!(
            md.starts_with("_session deadbeef · untitled_\nNightly:"),
            "{md}"
        );
        assert!(md.ends_with("**done**"), "{md}");
        assert!(
            !md.contains("graph LR") && !md.contains("```mermaid"),
            "{md}"
        );
    }
}
