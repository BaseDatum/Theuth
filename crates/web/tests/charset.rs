//! Design doc 122 D3: the decode order (BOM → `charset=` → `<meta>` → UTF-8 →
//! lossy) and the labels, plus the Shift_JIS byte fixture through the fetcher.

use web::fetch::charset::decode;
use web::fetch::{FetchFormat, Fetcher};
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const SHIFT_JIS_PAGE: &[u8] = include_bytes!("fixtures/shift-jis-page.bytes");

#[test]
fn charset_parameter_decodes_shift_jis() {
    let (text, label) = decode(SHIFT_JIS_PAGE, Some("text/html; charset=shift_jis"));
    assert_eq!(label, "shift_jis");
    assert!(text.contains("吾輩は猫である。名前はまだ無い。"), "{text}");
    // Quoted and mixed-case labels resolve too.
    let (_, label) = decode(SHIFT_JIS_PAGE, Some("text/html; charset=\"Shift_JIS\""));
    assert_eq!(label, "shift_jis");
}

#[test]
fn meta_charset_without_header_parameter() {
    let mut bytes = b"<html><head><meta charset=\"windows-1251\"><title>".to_vec();
    bytes.extend_from_slice(&[0xCF, 0xF0, 0xE8, 0xE2, 0xE5, 0xF2]); // "Привет"
    bytes.extend_from_slice(b"</title></head></html>");
    let (text, label) = decode(&bytes, Some("text/html"));
    assert_eq!(label, "windows-1251");
    assert!(text.contains("Привет"), "{text}");
    // http-equiv form.
    let mut bytes = b"<html><head><meta http-equiv=\"Content-Type\" content=\"text/html; charset=windows-1251\"></head><body>".to_vec();
    bytes.extend_from_slice(&[0xCF, 0xF0, 0xE8, 0xE2, 0xE5, 0xF2]);
    let (text, label) = decode(&bytes, None);
    assert_eq!(label, "windows-1251");
    assert!(text.contains("Привет"), "{text}");
    // A meta past the first 1 024 bytes is not consulted.
    let mut late = vec![b' '; 1100];
    late.extend_from_slice(b"<meta charset=\"windows-1251\">");
    late.extend_from_slice(&[0xCF, 0xF0]);
    assert_eq!(decode(&late, None).1, "utf-8");
}

#[test]
fn bom_wins_over_a_wrong_charset_parameter() {
    let mut bytes = vec![0xEF, 0xBB, 0xBF];
    bytes.extend_from_slice("Grüße".as_bytes());
    let (text, label) = decode(&bytes, Some("text/html; charset=shift_jis"));
    assert_eq!(label, "utf-8");
    assert_eq!(text, "Grüße");
    // UTF-16LE BOM.
    let mut bytes = vec![0xFF, 0xFE];
    for u in "hi".encode_utf16() {
        bytes.extend_from_slice(&u.to_le_bytes());
    }
    let (text, label) = decode(&bytes, None);
    assert_eq!((text.as_ref(), label), ("hi", "utf-16le"));
}

#[test]
fn bogus_label_and_invalid_utf8_fall_through() {
    let (text, label) = decode("plain".as_bytes(), Some("text/html; charset=x-nonsense"));
    assert_eq!((text.as_ref(), label), ("plain", "utf-8"));
    // The WHATWG `replacement` encoding label is treated as unknown.
    let (text, label) = decode("plain".as_bytes(), Some("text/html; charset=hz-gb-2312"));
    assert_eq!((text.as_ref(), label), ("plain", "utf-8"));
    let (text, label) = decode(&[b'a', 0xFF, b'b'], None);
    assert_eq!((text.as_ref(), label), ("a\u{FFFD}b", "utf-8"));
    // Valid UTF-8 with no hint borrows.
    let (text, label) = decode("héllo".as_bytes(), None);
    assert!(matches!(text, std::borrow::Cow::Borrowed(_)));
    assert_eq!((text.as_ref(), label), ("héllo", "utf-8"));
}

#[tokio::test]
async fn shift_jis_fixture_through_the_fetcher() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/neko"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(SHIFT_JIS_PAGE.to_vec(), "text/html; charset=shift_jis"),
        )
        .mount(&server)
        .await;
    let fetcher = Fetcher::new("theuth-test", 2_097_152, 5)
        .expect("client")
        .allowing(vec![*server.address()]);
    let r = fetcher
        .fetch(
            &format!("{}/neko", server.uri()),
            FetchFormat::Markdown,
            4000,
        )
        .await
        .expect("ok");
    assert_eq!(r.charset, "shift_jis");
    assert_eq!(r.title.as_deref(), Some("吾輩は猫である"));
    assert!(r.content.contains("名前はまだ無い"), "{}", r.content);
    assert_eq!(r.meta.lang.as_deref(), Some("ja"));
    assert_eq!(r.issue, None);
}
