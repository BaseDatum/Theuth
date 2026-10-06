//! `message_user` on Telegram (design docs 24 §5, 98 §8.10): the [`UserSink`]
//! the plugin registers with the agent crate's `"user"` actor. Bound sessions
//! get a message in their thread; the owner's chat is the fallback.

use agent::{Outbound, UserSink};

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
    /// `_<origin>_` as the first line (24 §2 step 2). Text as `send_rich`,
    /// plain chunks when Telegram rejects the markdown (98 §8.1), then one
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
            match chan.send(&to, &text, None, None, None).await {
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
