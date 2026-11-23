//! `web::fetch::download` (design doc 199 D16): a loopback HTTPS server with a
//! test CA, reached through the same exact-`ip:port` allowlist the fetcher
//! tests use, plus the test root; refusals need no network.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_rustls::rustls;
use tokio_rustls::rustls::pki_types::pem::PemObject;
use tokio_rustls::rustls::pki_types::{CertificateDer, PrivateKeyDer};
use web::fetch::{DownloadError, DownloadOpts, download};

type Conn = tokio_rustls::server::TlsStream<TcpStream>;
/// A URL or path, and the refusal it must produce.
type Case = (&'static str, fn(&DownloadError) -> bool);

const CA: &[u8] = include_bytes!("fixtures/tls/ca.pem");
const CERT: &[u8] = include_bytes!("fixtures/tls/server.pem");
const KEY: &[u8] = include_bytes!("fixtures/tls/server.key");

/// HTTPS on `127.0.0.1:0`; `handler(path, own address, connection)` answers each request.
async fn serve<F, Fut>(handler: F) -> SocketAddr
where
    F: Fn(String, SocketAddr, Conn) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = ()> + Send + 'static,
{
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .expect("versions")
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from_pem_slice(CERT).expect("cert")],
            PrivateKeyDer::from_pem_slice(KEY).expect("key"),
        )
        .expect("config");
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let addr = listener.local_addr().expect("addr");
    let handler = Arc::new(handler);
    tokio::spawn(async move {
        while let Ok((tcp, _)) = listener.accept().await {
            let (acceptor, handler) = (acceptor.clone(), Arc::clone(&handler));
            tokio::spawn(async move {
                let Ok(mut tls) = acceptor.accept(tcp).await else {
                    return;
                };
                let path = read_head(&mut tls).await;
                handler(path, addr, tls).await;
            });
        }
    });
    addr
}

/// Read the request head; the request path.
async fn read_head(tls: &mut Conn) -> String {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") && tls.read(&mut byte).await.unwrap_or(0) == 1 {
        head.push(byte[0]);
    }
    String::from_utf8_lossy(&head)
        .split_whitespace()
        .nth(1)
        .unwrap_or_default()
        .to_owned()
}

async fn send(tls: &mut Conn, bytes: &[u8]) {
    let _ = tls.write_all(bytes).await;
    let _ = tls.flush().await;
}

/// A complete response with `Content-Length`.
async fn reply(mut tls: Conn, status: &str, headers: &str, body: &[u8]) {
    let head = format!(
        "HTTP/1.1 {status}\r\n{headers}Content-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    send(&mut tls, head.as_bytes()).await;
    send(&mut tls, body).await;
    let _ = tls.shutdown().await;
}

async fn redirect(tls: Conn, location: &str) {
    reply(tls, "302 Found", &format!("Location: {location}\r\n"), b"").await;
}

/// Hold the connection open forever.
async fn hang(tls: Conn) {
    let _keep = tls;
    std::future::pending::<()>().await;
}

fn opts(addr: SocketAddr) -> DownloadOpts {
    DownloadOpts {
        test_allow: vec![addr],
        test_root_pem: Some(CA.to_vec()),
        ..DownloadOpts::default()
    }
}

fn url(addr: SocketAddr, path: &str) -> String {
    format!("https://{addr}{path}")
}

fn dest(dir: &tempfile::TempDir) -> PathBuf {
    dir.path().join("file.bin")
}

fn payload(n: usize) -> Vec<u8> {
    (0..n)
        .map(|i| u8::try_from(i % 251).expect("byte"))
        .collect()
}

fn assert_absent(path: &Path) {
    assert!(!path.exists(), "{} left behind", path.display());
}

#[tokio::test]
async fn content_length_file_is_hashed_and_written() {
    let body = payload(300_000);
    let served = body.clone();
    let addr = serve(move |_, _, tls| {
        let body = served.clone();
        async move {
            reply(tls, "200 OK", "Content-Type: Image/PNG; x=y\r\n", &body).await;
        }
    })
    .await;
    let dir = tempfile::tempdir().expect("tmp");
    let out = download(&url(addr, "/a.png"), &dest(&dir), &opts(addr))
        .await
        .expect("download");
    assert_eq!(out.bytes, body.len() as u64);
    assert_eq!(out.blake3, blake3::hash(&body).to_hex().to_string());
    assert_eq!(out.media_type.as_deref(), Some("image/png"));
    assert_eq!(out.final_url, url(addr, "/a.png"));
    assert_eq!(std::fs::read(dest(&dir)).expect("read"), body);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(dest(&dir))
            .expect("meta")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}

#[tokio::test]
async fn chunked_file_without_length_is_fetched() {
    let addr = serve(|_, _, mut tls| async move {
        send(
            &mut tls,
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        )
        .await;
        for chunk in [&b"hello "[..], b"chunked ", b"world"] {
            send(&mut tls, format!("{:x}\r\n", chunk.len()).as_bytes()).await;
            send(&mut tls, chunk).await;
            send(&mut tls, b"\r\n").await;
        }
        send(&mut tls, b"0\r\n\r\n").await;
    })
    .await;
    let dir = tempfile::tempdir().expect("tmp");
    let out = download(&url(addr, "/c"), &dest(&dir), &opts(addr))
        .await
        .expect("download");
    assert_eq!(out.bytes, 19);
    assert_eq!(
        out.blake3,
        blake3::hash(b"hello chunked world").to_hex().to_string()
    );
    assert_eq!(out.media_type, None);
}

#[tokio::test]
async fn static_refusals_write_nothing() {
    let dir = tempfile::tempdir().expect("tmp");
    let cases: &[Case] = &[
        ("http://example.com/f", |e| {
            matches!(e, DownloadError::NotHttps)
        }),
        ("ftp://example.com/f", |e| {
            matches!(e, DownloadError::NotHttps)
        }),
        ("file:///etc/passwd", |e| {
            matches!(e, DownloadError::NotHttps)
        }),
        ("https://user:pass@example.com/f", |e| {
            matches!(e, DownloadError::Userinfo)
        }),
        ("https://user@example.com/f", |e| {
            matches!(e, DownloadError::Userinfo)
        }),
        ("https://127.0.0.1/f", |e| {
            matches!(e, DownloadError::NotPublic)
        }),
        ("https://10.0.0.1/f", |e| {
            matches!(e, DownloadError::NotPublic)
        }),
        ("https://100.64.0.1/f", |e| {
            matches!(e, DownloadError::NotPublic)
        }),
        ("https://169.254.169.254/latest", |e| {
            matches!(e, DownloadError::NotPublic)
        }),
        ("https://[::ffff:127.0.0.1]/f", |e| {
            matches!(e, DownloadError::NotPublic)
        }),
        ("https://2130706433/f", |e| {
            matches!(e, DownloadError::NotPublic)
        }),
        ("https://0.0.0.0/f", |e| {
            matches!(e, DownloadError::NotPublic)
        }),
        ("https://[::1]/f", |e| matches!(e, DownloadError::NotPublic)),
        ("https://224.0.0.1/f", |e| {
            matches!(e, DownloadError::NotPublic)
        }),
        ("https://localhost/f", |e| {
            matches!(e, DownloadError::NotPublic)
        }),
        ("/relative", |e| matches!(e, DownloadError::InvalidUrl)),
    ];
    for (raw, expected) in cases {
        let err = download(raw, &dest(&dir), &DownloadOpts::default())
            .await
            .expect_err(raw);
        assert!(expected(&err), "{raw}: {err:?}");
        assert_absent(&dest(&dir));
    }
    assert_eq!(
        DownloadError::NotPublic.to_string(),
        "refused: not a public address"
    );
    assert_eq!(DownloadError::NotHttps.to_string(), "refused: not HTTPS");
    let big = DownloadError::TooLarge {
        max: 5_000_000_000,
        declared: None,
    };
    assert_eq!(big.to_string(), "too large: over 5.0 GB");
    let idle = DownloadError::IdleTimeout(Duration::from_secs(60));
    assert_eq!(idle.to_string(), "stalled: no data for 60 s");
}

#[tokio::test]
async fn redirects_are_followed_and_rechecked_per_hop() {
    let addr = serve(|path, addr, tls| async move {
        match path.as_str() {
            "/ok" => redirect(tls, "/file").await,
            "/to-http" => redirect(tls, &format!("http://{addr}/file")).await,
            "/to-private" => redirect(tls, "https://10.0.0.1/file").await,
            "/to-loopback" => redirect(tls, "https://127.0.0.1:1/file").await,
            "/loop" => redirect(tls, "/loop").await,
            "/file" => reply(tls, "200 OK", "", b"payload").await,
            _ => reply(tls, "404 Not Found", "", b"").await,
        }
    })
    .await;
    let dir = tempfile::tempdir().expect("tmp");
    let out = download(&url(addr, "/ok"), &dest(&dir), &opts(addr))
        .await
        .expect("redirected");
    assert_eq!(out.final_url, url(addr, "/file"));
    assert_eq!(out.bytes, 7);
    std::fs::remove_file(dest(&dir)).expect("rm");

    let cases: &[Case] = &[
        ("/to-http", |e| matches!(e, DownloadError::NotHttps)),
        ("/to-private", |e| matches!(e, DownloadError::NotPublic)),
        ("/to-loopback", |e| matches!(e, DownloadError::NotPublic)),
        ("/loop", |e| matches!(e, DownloadError::Redirects)),
        ("/missing", |e| matches!(e, DownloadError::HttpStatus(404))),
    ];
    for (path, expected) in cases {
        let err = download(&url(addr, path), &dest(&dir), &opts(addr))
            .await
            .expect_err(path);
        assert!(expected(&err), "{path}: {err:?}");
        assert_absent(&dest(&dir));
    }
}

#[tokio::test]
async fn declared_length_over_max_is_refused_before_the_body() {
    let addr = serve(|_, _, mut tls| async move {
        send(&mut tls, b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\n").await;
        hang(tls).await;
    })
    .await;
    let dir = tempfile::tempdir().expect("tmp");
    let o = DownloadOpts {
        max_bytes: 10,
        ..opts(addr)
    };
    let err = tokio::time::timeout(
        Duration::from_secs(5),
        download(&url(addr, "/"), &dest(&dir), &o),
    )
    .await
    .expect("refused without reading the body")
    .expect_err("too large");
    assert!(
        matches!(
            err,
            DownloadError::TooLarge {
                max: 10,
                declared: Some(1000)
            }
        ),
        "{err:?}"
    );
    assert_absent(&dest(&dir));
}

#[tokio::test]
async fn streamed_bytes_over_max_abort_and_remove_the_partial() {
    let addr = serve(|_, _, mut tls| async move {
        send(
            &mut tls,
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
        )
        .await;
        for _ in 0..8 {
            send(&mut tls, b"8\r\n01234567\r\n").await;
        }
        send(&mut tls, b"0\r\n\r\n").await;
    })
    .await;
    let dir = tempfile::tempdir().expect("tmp");
    let o = DownloadOpts {
        max_bytes: 10,
        ..opts(addr)
    };
    let err = download(&url(addr, "/"), &dest(&dir), &o)
        .await
        .expect_err("too large");
    assert!(
        matches!(
            err,
            DownloadError::TooLarge {
                max: 10,
                declared: None
            }
        ),
        "{err:?}"
    );
    assert_absent(&dest(&dir));
}

#[tokio::test]
async fn no_headers_ends_with_idle_timeout() {
    let addr = serve(|_, _, tls| hang(tls)).await;
    let dir = tempfile::tempdir().expect("tmp");
    let o = DownloadOpts {
        idle_timeout: Duration::from_millis(200),
        ..opts(addr)
    };
    let started = Instant::now();
    let err = download(&url(addr, "/"), &dest(&dir), &o)
        .await
        .expect_err("stalled");
    assert!(matches!(err, DownloadError::IdleTimeout(_)), "{err:?}");
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_absent(&dest(&dir));
}

/// Headers with `Content-Length: len`, then one byte every `every`.
async fn drip(mut tls: Conn, len: usize, every: Duration) {
    send(
        &mut tls,
        format!("HTTP/1.1 200 OK\r\nContent-Length: {len}\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await;
    for _ in 0..len {
        tokio::time::sleep(every).await;
        send(&mut tls, b"x").await;
    }
}

#[tokio::test]
async fn a_drip_ends_at_the_deadline() {
    let addr = serve(|_, _, tls| drip(tls, 200, Duration::from_millis(50))).await;
    let dir = tempfile::tempdir().expect("tmp");
    let o = DownloadOpts {
        idle_timeout: Duration::from_millis(300),
        deadline: Some(Instant::now() + Duration::from_millis(700)),
        ..opts(addr)
    };
    let err = download(&url(addr, "/"), &dest(&dir), &o)
        .await
        .expect_err("deadline");
    assert!(matches!(err, DownloadError::Deadline), "{err:?}");
    assert_absent(&dest(&dir));
}

#[tokio::test]
async fn a_transfer_longer_than_any_idle_budget_completes() {
    // 24 × 60 ms ≈ 1.4 s: nearly 5 idle budgets in all, never one without a byte.
    let addr = serve(|_, _, tls| drip(tls, 24, Duration::from_millis(60))).await;
    let dir = tempfile::tempdir().expect("tmp");
    let o = DownloadOpts {
        idle_timeout: Duration::from_millis(300),
        ..opts(addr)
    };
    let started = Instant::now();
    let out = download(&url(addr, "/"), &dest(&dir), &o)
        .await
        .expect("completes");
    assert!(started.elapsed() > o.idle_timeout * 4);
    assert_eq!(out.bytes, 24);
    assert_eq!(std::fs::read(dest(&dir)).expect("read"), vec![b'x'; 24]);
}

#[tokio::test]
async fn cancellation_removes_the_partial() {
    let addr = serve(|_, _, mut tls| async move {
        send(
            &mut tls,
            b"HTTP/1.1 200 OK\r\nContent-Length: 1000\r\n\r\npartial",
        )
        .await;
        hang(tls).await;
    })
    .await;
    let dir = tempfile::tempdir().expect("tmp");
    let path = dest(&dir);
    let o = opts(addr);
    let cancel = o.cancel.clone();
    let url = url(addr, "/");
    let task = tokio::spawn({
        let path = path.clone();
        async move { download(&url, &path, &o).await }
    });
    let waited = Instant::now();
    while !path.exists() {
        assert!(
            waited.elapsed() < Duration::from_secs(5),
            "file never created"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    cancel.cancel();
    let err = task.await.expect("join").expect_err("cancelled");
    assert!(matches!(err, DownloadError::Cancelled), "{err:?}");
    assert_absent(&path);
}

#[tokio::test]
async fn an_existing_dest_is_refused_and_kept() {
    let addr = serve(|_, _, tls| reply(tls, "200 OK", "", b"new")).await;
    let dir = tempfile::tempdir().expect("tmp");
    std::fs::write(dest(&dir), b"old").expect("seed");
    let err = download(&url(addr, "/"), &dest(&dir), &opts(addr))
        .await
        .expect_err("exists");
    assert!(matches!(err, DownloadError::Io(_)), "{err:?}");
    assert_eq!(std::fs::read(dest(&dir)).expect("read"), b"old");
}
