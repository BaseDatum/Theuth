//! Charset decoding (design doc 122 D3). Contract stub — lossy UTF-8 until W1
//! lands the BOM → `charset=` → `<meta charset>` → UTF-8 ladder on `encoding_rs`.

use std::borrow::Cow;

/// Decode `bytes` to text: BOM → `charset=` of `content_type` → `<meta charset>` /
/// `<meta http-equiv="Content-Type" content="…charset=…">` in the first 1 024 bytes → UTF-8 →
/// lossy UTF-8. Returns the text and the label actually used (`"utf-8"`, `"shift_jis"`, …).
///
/// Contract stub (122 §7.1): lossy UTF-8, label `"utf-8"`. The lifetime is
/// spelled out because the elided form in §7.2 is ambiguous between the two
/// borrowed arguments; the `Cow` borrows `bytes`.
#[must_use]
pub fn decode<'a>(bytes: &'a [u8], _content_type: Option<&str>) -> (Cow<'a, str>, &'static str) {
    (String::from_utf8_lossy(bytes), "utf-8")
}
