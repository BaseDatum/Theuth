//! The `asset` scheme, registered app-wide so Tauri skips its built-in handler
//! (`tauri-2.12.5/src/manager/webview.rs:357`, which only registers its own when the app has not). Tauri
//! registers every scheme on every webview and answers with `Access-Control-Allow-Origin` = the webview's own
//! origin (`protocol/asset.rs:13-24, 43`), so a web view tile's remote page could read the asset scope
//! (205 §5, G3). The only difference from Tauri's handler: a request from a `web-*` webview gets 403.
//!
//! Everything below `get_response` is a copy of `tauri-2.12.5/src/protocol/asset.rs` (not public in Tauri;
//! `protocol` is `pub(crate)`, `lib.rs:102`), with `log` → `tracing`, `getrandom` → `rand`,
//! `mime::TEXT_PLAIN` → its literal and the window origin computed per request the way
//! `manager/webview.rs:246-265` computes it at webview creation. Re-check it when Tauri is upgraded.

use std::borrow::Cow;
use std::io::SeekFrom;

use http_range::HttpRange;
use tauri::http::{Request, Response, header::*, status::StatusCode};
use tauri::path::SafePathBuf;
use tauri::scope::fs::Scope;
use tauri::utils::mime_type::MimeType;
use tauri::{Manager, Runtime, UriSchemeContext, UriSchemeResponder, Url};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};

/// The scheme name, Tauri's own.
pub const SCHEME: &str = "asset";

/// A web view tile (205 D3) may not read the asset scope.
fn denied(webview_label: &str) -> bool {
    webview_label.starts_with("web-")
}

/// `manager/webview.rs:246-265` of tauri 2.12.5 for a webview whose URL is `url` (this app never sets
/// `use_https_scheme`).
fn window_origin(url: &Url) -> String {
    if url.scheme() == "data" {
        "null".into()
    } else if (cfg!(windows) || cfg!(target_os = "android"))
        && url.scheme() != "http"
        && url.scheme() != "https"
    {
        // `protocol::origin(scheme, false)`
        format!("http://{}.localhost", url.scheme())
    } else if let Some(host) = url.host() {
        format!(
            "{}://{}{}",
            url.scheme(),
            host,
            url.port().map(|p| format!(":{p}")).unwrap_or_default()
        )
    } else {
        "null".into()
    }
}

/// The app's own URL, for a request that arrives before its webview is registered with the manager: every
/// non-`web-*` webview of this app loads a `WebviewUrl::App` route.
fn app_url<R: Runtime>(app: &tauri::AppHandle<R>) -> Url {
    if tauri::is_dev()
        && let Some(dev) = app.config().build.dev_url.clone()
    {
        return dev;
    }
    let url = if cfg!(windows) || cfg!(target_os = "android") {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    };
    Url::parse(url).expect("static url")
}

/// `Builder::register_asynchronous_uri_scheme_protocol` handler.
pub fn handler<R: Runtime>(
    ctx: UriSchemeContext<'_, R>,
    request: Request<Vec<u8>>,
    responder: UriSchemeResponder,
) {
    let app = ctx.app_handle();
    let label = ctx.webview_label();
    if denied(label) {
        responder.respond(
            Response::builder()
                .status(StatusCode::FORBIDDEN)
                .body(Vec::new())
                .unwrap(),
        );
        return;
    }
    let url = app
        .get_webview(label)
        .and_then(|w| w.url().ok())
        .unwrap_or_else(|| app_url(app));
    let window_origin = window_origin(&url);
    let scope = app.asset_protocol_scope();
    tauri::async_runtime::spawn(async move {
        match get_response(request, &scope, &window_origin).await {
            Ok(response) => responder.respond(response),
            Err(e) => responder.respond(
                Response::builder()
                    .status(StatusCode::INTERNAL_SERVER_ERROR)
                    .header(CONTENT_TYPE, "text/plain")
                    .header("Access-Control-Allow-Origin", &window_origin)
                    .body(e.to_string().into_bytes())
                    .unwrap(),
            ),
        }
    });
}

async fn get_response(
    request: Request<Vec<u8>>,
    scope: &Scope,
    window_origin: &str,
) -> Result<Response<Cow<'static, [u8]>>, Box<dyn std::error::Error>> {
    // skip leading `/`
    let path = request.uri().path().as_bytes();
    let path = percent_encoding::percent_decode(path.strip_prefix(b"/").unwrap_or(path))
        .decode_utf8_lossy();

    let mut resp = Response::builder().header("Access-Control-Allow-Origin", window_origin);

    let safe_path = match SafePathBuf::new(path.as_ref().into()) {
        Ok(path) => path,
        Err(e) => {
            tracing::error!("asset protocol path \"{path}\" is not valid: {e}");
            return resp.status(403).body(Vec::new().into()).map_err(Into::into);
        }
    };

    if !scope.is_allowed(&safe_path) {
        tracing::error!("asset protocol not configured to allow the path: {path}");
        return resp.status(403).body(Vec::new().into()).map_err(Into::into);
    }

    // Separate block for easier error handling
    let mut file = match File::open(&safe_path).await {
        Ok(file) => file,
        Err(e) => {
            return if e.kind() == std::io::ErrorKind::NotFound {
                tracing::error!("File does not exist at path: {path}");
                resp.status(404).body(Vec::new().into()).map_err(Into::into)
            } else if e.kind() == std::io::ErrorKind::PermissionDenied {
                tracing::error!("Missing OS permission to access path \"{path}\": {e}");
                resp.status(403).body(Vec::new().into()).map_err(Into::into)
            } else {
                Err(e.into())
            };
        }
    };

    let len = file.metadata().await?.len();
    let (mime_type, read_bytes) = {
        // get file mime type
        let nbytes = len.min(8192);
        let mut magic_buf = Vec::with_capacity(nbytes as usize);
        (&mut file).take(nbytes).read_to_end(&mut magic_buf).await?;
        file.rewind().await?;
        (
            MimeType::parse(&magic_buf, &path),
            // return the `magic_bytes` if we read the whole file
            // to avoid reading it again later if this is not a range request
            if len < 8192 { Some(magic_buf) } else { None },
        )
    };

    resp = resp.header(CONTENT_TYPE, &mime_type);

    // handle 206 (partial range) http requests
    let response = if let Some(range_header) =
        request.headers().get("range").and_then(|r| r.to_str().ok())
    {
        resp = resp.header(ACCEPT_RANGES, "bytes");
        resp = resp.header(ACCESS_CONTROL_EXPOSE_HEADERS, "content-range");

        let not_satisfiable = || {
            Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(CONTENT_RANGE, format!("bytes */{len}"))
                .body(vec![].into())
                .map_err(Into::into)
        };

        // parse range header
        let Some(ranges) = parse_ranges(range_header, len) else {
            return not_satisfiable();
        };

        // single-part range header
        if ranges.len() == 1 {
            let &(start, mut end) = ranges.first().unwrap();

            // check if a range is not satisfiable
            //
            // this should be already taken care of by the range parsing library
            // but checking here again for extra assurance
            if start >= len || end >= len || end < start {
                return not_satisfiable();
            }

            // adjust end byte for MAX_LEN
            end = clamp_end(start, end, len);

            // calculate number of bytes needed to be read
            let nbytes = end + 1 - start;

            let buf = {
                let mut buf = Vec::with_capacity(nbytes as usize);
                file.seek(SeekFrom::Start(start)).await?;
                file.take(nbytes).read_to_end(&mut buf).await?;
                buf
            };

            resp = resp.header(CONTENT_RANGE, format!("bytes {start}-{end}/{len}"));
            resp = resp.header(CONTENT_LENGTH, end + 1 - start);
            resp = resp.status(StatusCode::PARTIAL_CONTENT);
            resp.body(buf.into())
        } else {
            let ranges = ranges
                .iter()
                .filter_map(|&(start, end)| {
                    // filter out unsatisfiable ranges
                    //
                    // this should be already taken care of by the range parsing library
                    // but checking here again for extra assurance
                    if start >= len || end >= len || end < start {
                        None
                    } else {
                        // adjust end byte for MAX_LEN
                        Some((start, clamp_end(start, end, len)))
                    }
                })
                .collect::<Vec<_>>();

            let boundary = random_boundary();
            let boundary_sep = format!("\r\n--{boundary}\r\n");
            let boundary_closer = format!("\r\n--{boundary}--\r\n");

            // `Builder::header` appends, we want to replace the file mime type set earlier
            if let Some(headers) = resp.headers_mut() {
                headers.insert(
                    CONTENT_TYPE,
                    HeaderValue::from_str(&format!("multipart/byteranges; boundary={boundary}"))?,
                );
            }

            let buf = {
                // multi-part range header
                let mut buf = Vec::new();

                for (start, end) in ranges {
                    // a new range is being written, write the range boundary
                    buf.write_all(boundary_sep.as_bytes()).await?;

                    // write the needed headers `Content-Type` and `Content-Range`
                    buf.write_all(format!("{CONTENT_TYPE}: {mime_type}\r\n").as_bytes())
                        .await?;
                    buf.write_all(
                        format!("{CONTENT_RANGE}: bytes {start}-{end}/{len}\r\n").as_bytes(),
                    )
                    .await?;

                    // write the separator to indicate the start of the range body
                    buf.write_all("\r\n".as_bytes()).await?;

                    // calculate number of bytes needed to be read
                    let nbytes = end + 1 - start;

                    buf.reserve(nbytes as usize);
                    file.seek(SeekFrom::Start(start)).await?;
                    (&mut file).take(nbytes).read_to_end(&mut buf).await?;
                }
                // all ranges have been written, write the closing boundary
                buf.write_all(boundary_closer.as_bytes()).await?;

                buf
            };

            resp = resp.status(StatusCode::PARTIAL_CONTENT);
            resp.body(buf.into())
        }
    } else if request.method() == tauri::http::Method::HEAD {
        // if the HEAD method is used, we should not return a body
        resp = resp.header(CONTENT_LENGTH, len);
        resp.body(Vec::new().into())
    } else {
        // avoid reading the file if we already read it
        // as part of mime type detection
        let buf = if let Some(b) = read_bytes {
            b
        } else {
            let mut local_buf = Vec::with_capacity(len as usize);
            file.read_to_end(&mut local_buf).await?;
            local_buf
        };
        resp = resp.header(CONTENT_LENGTH, len);
        resp.body(buf.into())
    };

    response.map_err(Into::into)
}

/// The maximum bytes sent for one range.
const MAX_LEN: u64 = 1000 * 1024;

/// `Range` header → inclusive `(start, end)` pairs, `None` when not satisfiable.
fn parse_ranges(header: &str, len: u64) -> Option<Vec<(u64, u64)>> {
    HttpRange::parse(header, len).ok().map(|ranges| {
        ranges
            .iter()
            // map the output to spec range <start-end>, example: 0-499
            .map(|r| (r.start, r.start + r.length - 1))
            .collect()
    })
}

/// Clamp a satisfiable range's end to `MAX_LEN` bytes.
fn clamp_end(start: u64, end: u64, len: u64) -> u64 {
    start + (end - start).min(len - start).min(MAX_LEN - 1)
}

fn random_boundary() -> String {
    let x: [u8; 30] = rand::random();
    x.iter().fold(String::new(), |mut a, b| {
        a.push_str(&format!("{b:x}"));
        a
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webview_asset_denied_only_for_web_views() {
        assert!(denied("web-0123456789abcdef"));
        for label in ["main", "island", "popout-web-0123456789abcdef", "overlay-1"] {
            assert!(!denied(label), "{label}");
        }
    }

    #[test]
    fn webview_asset_window_origin_matches_tauri() {
        let o = |s: &str| window_origin(&Url::parse(s).unwrap());
        assert_eq!(o("http://localhost:1420/sessions"), "http://localhost:1420");
        assert_eq!(o("https://example.com/a"), "https://example.com");
        assert_eq!(o("data:text/html,x"), "null");
        if cfg!(windows) {
            assert_eq!(o("tauri://localhost/index.html"), "http://tauri.localhost");
        } else {
            assert_eq!(o("tauri://localhost/index.html"), "tauri://localhost");
        }
    }

    #[test]
    fn webview_asset_range_parsing() {
        assert_eq!(parse_ranges("bytes=0-499", 1000), Some(vec![(0, 499)]));
        assert_eq!(parse_ranges("bytes=500-", 1000), Some(vec![(500, 999)]));
        assert_eq!(parse_ranges("bytes=-100", 1000), Some(vec![(900, 999)]));
        assert_eq!(
            parse_ranges("bytes=0-9, 20-29", 1000),
            Some(vec![(0, 9), (20, 29)])
        );
        assert_eq!(parse_ranges("bytes=2000-3000", 1000), None);
        assert_eq!(parse_ranges("lines=1-2", 1000), None);
        // MAX_LEN clamp: 1000 KiB per range.
        assert_eq!(clamp_end(0, 9_999_999, 10_000_000), MAX_LEN - 1);
        assert_eq!(clamp_end(10, 19, 1000), 19);
    }
}
