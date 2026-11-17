//! 199 admission (Text first, then notices): a message's file reference parts become one
//! `in` row each, written beside the exchange before its text is delivered, so
//! `PeerRecord.files` carries the `state="receiving"` placeholders. The a2a layer checked
//! each part already (URL shape, host, size, digest, expiry); what does not read back here
//! is skipped, never guessed.

use a2a_types::{Part, PartContent};
use chrono::{DateTime, Utc};

use super::{FILE_MAX, OFFER_TTL, sanitise};
use crate::store::Store;
use crate::store::contact_files::FileRow;

/// The files extension's metadata key (`a2a::contacts::EXT_FILES`; a2a depends on this
/// crate, so the URI is spelled here too).
const EXT_FILES: &str = "https://theuth.io/a2a/ext/files/v1";

/// `(member, sender's id)` of a reference URL `iroh://<member>/a2a/files/<id>`.
pub fn source_of(url: &str) -> Option<(String, String)> {
    let rest = url.strip_prefix("iroh://")?;
    let (host, id) = rest.split_once("/a2a/files/")?;
    let hex = |s: &str, n: Option<usize>| {
        !s.is_empty()
            && n.is_none_or(|n| s.len() == n)
            && s.bytes()
                .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
    };
    (hex(host, None) && hex(id, Some(32))).then(|| (host.to_owned(), id.to_owned()))
}

/// `size`, `blake3` and `expires` of a reference part (a ProtoJSON `size` is a double).
fn meta(p: &Part) -> Option<(u64, String, DateTime<Utc>)> {
    let m = p.metadata.as_ref()?.get(EXT_FILES)?;
    let size = m.get("size")?;
    let size = size.as_u64().or_else(|| {
        size.as_f64()
            .filter(|f| *f >= 0.0 && f.fract() == 0.0 && *f <= FILE_MAX as f64)
            .map(|f| f as u64)
    })?;
    let blake3 = m.get("blake3")?.as_str()?;
    let hex = |b: u8| matches!(b, b'0'..=b'9' | b'a'..=b'f');
    if size > FILE_MAX || blake3.len() != 64 || !blake3.bytes().all(hex) {
        return None;
    }
    let expires = m.get("expires")?.as_str()?.parse().ok()?;
    Some((size, blake3.to_owned(), expires))
}

/// The `in` rows of `parts` for `session`'s exchange `exchange` from contact record
/// `contact`, at `now`: one per reference part, in order; the local deadline is
/// `min(declared expiry, now + OFFER_TTL)` (D8).
pub fn in_rows(
    session: &str,
    exchange: &str,
    contact: &str,
    parts: &[Part],
    now: DateTime<Utc>,
) -> Vec<FileRow> {
    let ttl = now + chrono::Duration::from_std(OFFER_TTL).unwrap_or_default();
    parts
        .iter()
        .filter_map(|p| {
            let PartContent::Url(url) = &p.content else {
                return None;
            };
            source_of(url)?;
            let (size, blake3, expires) = meta(p)?;
            Some((p, url, size, blake3, expires))
        })
        .enumerate()
        .map(|(seq, (p, url, size, blake3, expires))| FileRow {
            id: uuid::Uuid::new_v4().simple().to_string(),
            direction: "in".into(),
            session: session.to_owned(),
            exchange: exchange.to_owned(),
            seq: seq as i64,
            contact_id: contact.to_owned(),
            name: sanitise(p.filename.as_deref().unwrap_or("file")),
            media_type: p
                .media_type
                .clone()
                .filter(|m| !m.trim().is_empty())
                .unwrap_or_else(|| "application/octet-stream".into()),
            size: Some(size as i64),
            blake3: Some(blake3),
            source: url.clone(),
            state: "queued".into(),
            created_ms: now.timestamp_millis(),
            deadline_ms: expires.min(ttl).timestamp_millis(),
            ..Default::default()
        })
        .collect()
}

/// Write the `in` rows of a contact's message (sender `peer:<binding>`) into exchange
/// `exchange` of `session`, once. A sender with no contact record carries no files here.
pub async fn admit(
    store: &Store,
    session: runtime::SessionId,
    exchange: uuid::Uuid,
    sender: &str,
    parts: &[Part],
) -> anyhow::Result<()> {
    if !parts
        .iter()
        .any(|p| matches!(p.content, PartContent::Url(_)))
    {
        return Ok(());
    }
    let Some((contact, _)) = store.contact_of_binding(sender).await? else {
        tracing::warn!(%exchange, "199: files from a sender with no contact record dropped");
        return Ok(());
    };
    let rows = in_rows(
        &session.to_string(),
        &exchange.to_string(),
        &contact,
        parts,
        Utc::now(),
    );
    if store.admit_in_files(&exchange.to_string(), &rows).await? {
        #[cfg(unix)]
        super::service::kick();
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn part(url: &str, size: serde_json::Value, name: Option<&str>) -> Part {
        let mut p = Part::url(url);
        p.filename = name.map(str::to_owned);
        p.media_type = Some("application/pdf".into());
        p.metadata = Some(
            [(
                EXT_FILES.to_owned(),
                serde_json::json!({
                    "size": size,
                    "blake3": "a".repeat(64),
                    "expires": "2026-11-24T14:32:00Z",
                }),
            )]
            .into(),
        );
        p
    }

    /// One row per reference part, in order, with a ProtoJSON double size, the sanitised
    /// name and the shorter of the declared expiry and the local TTL; anything that does
    /// not read back is skipped.
    #[test]
    fn reference_parts_become_queued_rows() {
        let url = format!("iroh://abc123/a2a/files/{}", "0".repeat(32));
        let now: DateTime<Utc> = "2026-11-17T00:00:00Z".parse().unwrap();
        let parts = vec![
            Part::text("hi"),
            part(
                &url,
                serde_json::json!(2516582.0),
                Some("../../etc/pa\u{202e}ss"),
            ),
            part("https://x/a2a/files/0", serde_json::json!(1), None),
            part(&url, serde_json::json!(FILE_MAX + 1), None),
            part(&url, serde_json::json!(0), None),
        ];
        let rows = in_rows("s", "e", "rec", &parts, now);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].size, Some(2516582));
        assert_eq!(rows[0].name, "pass");
        assert_eq!((rows[0].seq, rows[1].seq), (0, 1));
        assert_eq!(rows[1].name, "file");
        assert_eq!(rows[0].state, "queued");
        let expires: DateTime<Utc> = "2026-11-24T00:00:00Z".parse().unwrap();
        assert_eq!(rows[0].deadline_ms, expires.timestamp_millis());
        assert_ne!(rows[0].id, rows[1].id);
    }
}
