//! Charset decoding (design doc 122 D3): BOM → `charset=` of the
//! `Content-Type` → `<meta charset>` / `http-equiv` in the first 1 024 bytes
//! → UTF-8 → lossy UTF-8, on `encoding_rs`.

use std::borrow::Cow;

use encoding_rs::{Encoding, REPLACEMENT, UTF_8};

/// How far into the body a `<meta charset>` is looked for.
const META_SCAN: usize = 1024;

/// Decode `bytes` to text: BOM → `charset=` of `content_type` → `<meta charset>` /
/// `<meta http-equiv="Content-Type" content="…charset=…">` in the first 1 024 bytes → UTF-8 →
/// lossy UTF-8. Returns the text and the label actually used (`"utf-8"`, `"shift_jis"`, …).
///
/// An unknown label (or the WHATWG `replacement` encoding) falls through to
/// UTF-8. The lifetime is spelled out because the elided form in §7.2 is
/// ambiguous between the two borrowed arguments; the `Cow` borrows `bytes`.
#[must_use]
pub fn decode<'a>(bytes: &'a [u8], content_type: Option<&str>) -> (Cow<'a, str>, &'static str) {
    if let Some((enc, bom)) = Encoding::for_bom(bytes) {
        return with(enc, &bytes[bom..]);
    }
    let hinted = content_type
        .and_then(charset_param)
        .or_else(|| meta_charset(&bytes[..bytes.len().min(META_SCAN)]))
        .and_then(|label| Encoding::for_label(label.as_bytes()))
        .filter(|enc| *enc != REPLACEMENT && *enc != UTF_8);
    if let Some(enc) = hinted {
        return with(enc, bytes);
    }
    (String::from_utf8_lossy(bytes), "utf-8")
}

fn with<'a>(enc: &'static Encoding, bytes: &'a [u8]) -> (Cow<'a, str>, &'static str) {
    if enc == UTF_8 {
        return (String::from_utf8_lossy(bytes), "utf-8");
    }
    let (text, _malformed) = enc.decode_without_bom_handling(bytes);
    (text, label(enc))
}

/// Lowercase label of `enc`; `encoding_rs` names are mixed-case
/// (`Shift_JIS`, `EUC-JP`, `Big5`) and the response field is documented lowercase.
fn label(enc: &'static Encoding) -> &'static str {
    match enc.name() {
        "UTF-8" => "utf-8",
        "UTF-16BE" => "utf-16be",
        "UTF-16LE" => "utf-16le",
        "Shift_JIS" => "shift_jis",
        "EUC-JP" => "euc-jp",
        "ISO-2022-JP" => "iso-2022-jp",
        "EUC-KR" => "euc-kr",
        "Big5" => "big5",
        "GBK" => "gbk",
        "KOI8-R" => "koi8-r",
        "KOI8-U" => "koi8-u",
        "IBM866" => "ibm866",
        "ISO-8859-2" => "iso-8859-2",
        "ISO-8859-3" => "iso-8859-3",
        "ISO-8859-4" => "iso-8859-4",
        "ISO-8859-5" => "iso-8859-5",
        "ISO-8859-6" => "iso-8859-6",
        "ISO-8859-7" => "iso-8859-7",
        "ISO-8859-8" => "iso-8859-8",
        "ISO-8859-8-I" => "iso-8859-8-i",
        "ISO-8859-10" => "iso-8859-10",
        "ISO-8859-13" => "iso-8859-13",
        "ISO-8859-14" => "iso-8859-14",
        "ISO-8859-15" => "iso-8859-15",
        "ISO-8859-16" => "iso-8859-16",
        // `windows-125x`, `gb18030`, `macintosh`, `x-mac-cyrillic`, `x-user-defined` are already lowercase.
        other => other,
    }
}

/// The `charset=` parameter of a `Content-Type` value, unquoted.
fn charset_param(content_type: &str) -> Option<String> {
    content_type.split(';').skip(1).find_map(|p| {
        let (k, v) = p.split_once('=')?;
        k.trim()
            .eq_ignore_ascii_case("charset")
            .then(|| v.trim().trim_matches(|c| c == '"' || c == '\'').to_owned())
    })
}

/// `charset=<label>` inside a `<meta …>` tag of `head` (covers both
/// `<meta charset="x">` and `http-equiv` `content="text/html; charset=x"`).
fn meta_charset(head: &[u8]) -> Option<String> {
    let lower = String::from_utf8_lossy(head).to_ascii_lowercase();
    let mut rest = lower.as_str();
    while let Some(open) = rest.find("<meta") {
        let tag = &rest[open..];
        let end = tag.find('>').unwrap_or(tag.len());
        let tag = &tag[..end];
        if let Some(i) = tag.find("charset=") {
            let label: String = tag[i + "charset=".len()..]
                .trim_start_matches(|c| c == '"' || c == '\'' || c == ' ')
                .chars()
                .take_while(|c| !matches!(c, '"' | '\'' | ';' | ' ' | '/' | '>'))
                .collect();
            if !label.is_empty() {
                return Some(label);
            }
        }
        rest = &rest[open + end..];
    }
    None
}
