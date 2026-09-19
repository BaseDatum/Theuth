//! Project label normalization (design doc 51 §3). One rule, applied once at
//! the API boundary: no control characters, trimmed, 1–128 UTF-8 bytes; case
//! and interior spaces are kept. No slug, no path or cwd inference.

/// Longest accepted label in UTF-8 bytes after trimming.
pub const MAX_BYTES: usize = 128;

/// Normalize a project label. `None` clears (`Ok(None)`); `Some` must have
/// no control characters and trim to 1..=[`MAX_BYTES`] bytes.
pub fn normalize_project(input: Option<&str>) -> Result<Option<String>, String> {
    let Some(raw) = input else {
        return Ok(None);
    };
    if raw.chars().any(char::is_control) {
        return Err("project must not contain control characters".into());
    }
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("project must not be empty".into());
    }
    if trimmed.len() > MAX_BYTES {
        return Err(format!("project must be at most {MAX_BYTES} bytes"));
    }
    Ok(Some(trimmed.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trims_then_checks_byte_boundaries() {
        let ok = |n: usize| normalize_project(Some(&format!("  {} \u{3000}", "é".repeat(n / 2))));
        assert_eq!(ok(126).unwrap().unwrap().len(), 126);
        assert_eq!(ok(128).unwrap().unwrap().len(), 128);
        assert!(ok(130).is_err());
        assert_eq!(
            normalize_project(Some(&"a".repeat(129))).unwrap_err(),
            "project must be at most 128 bytes"
        );
        assert_eq!(normalize_project(Some(&"a".repeat(128))).unwrap().unwrap().len(), 128);
        // Whitespace beyond the limit is fine: the limit applies after trimming.
        let padded = format!("{}{}", "a".repeat(127), " ".repeat(10));
        assert_eq!(normalize_project(Some(&padded)).unwrap().unwrap().len(), 127);
    }

    #[test]
    fn rejects_controls_empty_and_keeps_case() {
        for bad in ["a\0b", "a\tb", "a\nb", "a\rb", "a\u{7f}b", "\u{85}x", "x\u{9b}"] {
            assert!(normalize_project(Some(bad)).is_err(), "{bad:?}");
        }
        assert!(normalize_project(Some("")).is_err());
        assert!(normalize_project(Some("   \u{a0}\u{2003}")).is_err());
        assert_eq!(normalize_project(None), Ok(None));
        assert_eq!(
            normalize_project(Some("  My  Big Project ")).unwrap().as_deref(),
            Some("My  Big Project")
        );
    }
}
