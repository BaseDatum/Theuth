//! Inbound attachment helpers: magic-byte MIME sniffing and the extension for a MIME.

/// The MIME of the formats chats send, from their magic bytes.
#[must_use]
pub fn sniff(b: &[u8]) -> Option<&'static str> {
    if b.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        Some("image/png")
    } else if b.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if b.starts_with(b"GIF87a") || b.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if b.len() >= 12 && b.starts_with(b"RIFF") && &b[8..12] == b"WEBP" {
        Some("image/webp")
    } else if b.starts_with(b"%PDF") {
        Some("application/pdf")
    } else if b.starts_with(b"OggS") {
        Some("audio/ogg")
    } else {
        None
    }
}

/// A file extension for `mime`: the known renames, else its subtype, else `bin`.
#[must_use]
pub fn extension_for(mime: &str) -> String {
    match mime {
        "image/jpeg" => "jpg".into(),
        "text/plain" => "txt".into(),
        "audio/mpeg" => "mp3".into(),
        "audio/mp4" => "m4a".into(),
        "application/octet-stream" => "bin".into(),
        _ => {
            let sub: String = mime
                .rsplit('/')
                .next()
                .unwrap_or_default()
                .chars()
                .take_while(|c| c.is_ascii_alphanumeric())
                .collect();
            if sub.is_empty() { "bin".into() } else { sub }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mime_helpers() {
        assert_eq!(sniff(b"%PDF-1.4"), Some("application/pdf"));
        assert_eq!(sniff(b"zzz"), None);
        assert_eq!(extension_for("image/jpeg"), "jpg");
        assert_eq!(extension_for("application/pdf"), "pdf");
        assert_eq!(extension_for("weird"), "weird");
    }
}
