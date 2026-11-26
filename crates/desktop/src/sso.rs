//! 221 §9.6 Bitwarden SSO: the one-shot loopback listener the browser comes back to. [`sso_listen`]
//! binds `localhost` (127.0.0.1 and ::1: the browser may resolve either) on the first free port of
//! [`PORTS`], the only redirect URIs Bitwarden's `cli` client allows; the webview passes that port to
//! `connection_sso_begin` and opens `authorize_url`. [`sso_wait`] takes the first
//! `GET /?code=…&state=…` (or `?error=…`), answers oauth's callback page ("Logged in" / "Login
//! failed") and returns the whole
//! redirect URL for `connection_sso_complete`. Any other path or method is refused and the wait goes
//! on (a browser asks for `/favicon.ico` too). [`TIMEOUT`] or [`sso_cancel`] ends it. The code and the
//! query never reach `tracing`.

use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr};
use std::ops::RangeInclusive;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Notify, mpsc};

/// The ports Bitwarden's `cli` client accepts as `http://localhost:<port>` redirect URIs.
pub const PORTS: RangeInclusive<u16> = 8065..=8070;

/// How long the browser has to come back.
pub const TIMEOUT: Duration = Duration::from_secs(300);

/// A connection that sends no request line in this long is dropped (a browser's preconnect).
const READ_TIMEOUT: Duration = Duration::from_secs(10);

/// The largest request head read.
const MAX_HEAD: usize = 16 * 1024;

/// The bound sockets of one login and the port they share.
pub struct Bound {
    pub port: u16,
    v4: TcpListener,
    v6: Option<TcpListener>,
}

/// Bind 127.0.0.1 and, where the host has IPv6, ::1 on the first port of `ports` free on both.
pub fn bind(ports: RangeInclusive<u16>) -> io::Result<Bound> {
    let mut last = io::Error::new(io::ErrorKind::AddrInUse, "no port to try");
    for port in ports {
        let v4 = match std::net::TcpListener::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, port))) {
            Ok(l) => l,
            Err(e) => {
                last = e;
                continue;
            }
        };
        let port = v4.local_addr()?.port();
        let v6 = match std::net::TcpListener::bind(SocketAddr::from((Ipv6Addr::LOCALHOST, port))) {
            Ok(l) => Some(l),
            // No IPv6 loopback on this host: `localhost` then resolves to 127.0.0.1 only.
            Err(e) if e.kind() == io::ErrorKind::AddrNotAvailable => None,
            Err(e) => {
                last = e;
                continue;
            }
        };
        let tokio = |l: std::net::TcpListener| -> io::Result<TcpListener> {
            l.set_nonblocking(true)?;
            TcpListener::from_std(l)
        };
        return Ok(Bound {
            port,
            v4: tokio(v4)?,
            v6: v6.map(tokio).transpose()?,
        });
    }
    Err(last)
}

/// Wait for the browser's redirect on `bound` until `timeout` or `cancel`; `Ok` is the full
/// `http://localhost:<port>/?…` URL.
pub async fn accept_redirect(
    bound: Bound,
    timeout: Duration,
    cancel: Arc<Notify>,
) -> Result<String, String> {
    let (tx, mut rx) = mpsc::channel::<String>(1);
    let port = bound.port;
    let accept = async {
        loop {
            let next = tokio::select! {
                r = bound.v4.accept() => r,
                r = async {
                    match &bound.v6 {
                        Some(l) => l.accept().await,
                        None => std::future::pending().await,
                    }
                } => r,
            };
            // A failed accept (the peer reset before we took it) is not the login's failure.
            if let Ok((stream, _)) = next {
                tokio::spawn(answer(stream, port, tx.clone()));
            }
        }
    };
    tokio::select! {
        url = rx.recv() => url.ok_or_else(|| "the SSO listener closed".to_owned()),
        () = accept => unreachable!("the accept loop never ends"),
        () = tokio::time::sleep(timeout) => Err(format!("no answer from the browser within {} minutes", timeout.as_secs().div_ceil(60))),
        () = cancel.notified() => Err("SSO login cancelled".to_owned()),
    }
}

/// Serve one connection: the redirect gets the closing page and goes to `tx`; anything else is
/// refused with its status.
async fn answer(mut stream: TcpStream, port: u16, tx: mpsc::Sender<String>) {
    let Ok(Ok(head)) = tokio::time::timeout(READ_TIMEOUT, read_head(&mut stream)).await else {
        return;
    };
    let (status, body, url) = match classify(&head) {
        Verdict::Redirect { target, error } => (
            "200 OK",
            oauth::callback_page(!error),
            Some(format!("http://localhost:{port}{target}")),
        ),
        Verdict::Refused(status) => (status, String::new(), None),
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = stream.write_all(response.as_bytes()).await;
    let _ = stream.shutdown().await;
    if let Some(url) = url {
        let _ = tx.try_send(url);
    }
}

async fn read_head(stream: &mut TcpStream) -> io::Result<String> {
    let mut buf = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
        if buf.len() > MAX_HEAD {
            return Err(io::ErrorKind::InvalidData.into());
        }
        let n = stream.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    String::from_utf8(buf).map_err(|_| io::ErrorKind::InvalidData.into())
}

#[derive(Debug, PartialEq)]
enum Verdict<'a> {
    /// `GET /?code=…&state=…` or `GET /?error=…`; `target` is the request target as sent.
    Redirect {
        target: &'a str,
        error: bool,
    },
    Refused(&'static str),
}

fn classify(head: &str) -> Verdict<'_> {
    let line = head.lines().next().unwrap_or_default();
    let mut parts = line.split(' ');
    let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
        return Verdict::Refused("400 Bad Request");
    };
    if method != "GET" {
        return Verdict::Refused("405 Method Not Allowed");
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != "/" {
        return Verdict::Refused("404 Not Found");
    }
    let has = |key: &str| {
        query
            .split('&')
            .any(|kv| kv.split_once('=').map_or(kv, |(k, _)| k) == key)
    };
    if has("error") {
        Verdict::Redirect {
            target,
            error: true,
        }
    } else if has("code") && has("state") {
        Verdict::Redirect {
            target,
            error: false,
        }
    } else {
        Verdict::Refused("400 Bad Request")
    }
}

/// The login in progress: its sockets until [`sso_wait`] takes them, and its cancel signal.
#[derive(Default)]
pub struct Sso(Mutex<Option<Pending>>);

struct Pending {
    port: u16,
    bound: Option<Bound>,
    cancel: Arc<Notify>,
}

impl Sso {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<Pending>> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Bind the listener; returns the port for `connection_sso_begin`. A login already waiting is
/// cancelled first (one at a time).
#[tauri::command]
pub async fn sso_listen(state: tauri::State<'_, Sso>) -> Result<u16, String> {
    if let Some(old) = state.lock().take() {
        old.cancel.notify_one();
    }
    let bound = bind(PORTS).map_err(|e| {
        format!(
            "no free port in {}–{} for the SSO answer: {e}",
            PORTS.start(),
            PORTS.end()
        )
    })?;
    let port = bound.port;
    *state.lock() = Some(Pending {
        port,
        bound: Some(bound),
        cancel: Arc::new(Notify::new()),
    });
    Ok(port)
}

/// Wait for the browser on the listener [`sso_listen`] bound on `port`; the full redirect URL.
#[tauri::command]
pub async fn sso_wait(state: tauri::State<'_, Sso>, port: u16) -> Result<String, String> {
    let (bound, cancel) = {
        let mut slot = state.lock();
        match slot.as_mut() {
            Some(p) if p.port == port => (p.bound.take(), p.cancel.clone()),
            _ => (None, Arc::new(Notify::new())),
        }
    };
    let bound = bound.ok_or_else(|| "no SSO listener on that port".to_owned())?;
    let out = accept_redirect(bound, TIMEOUT, cancel.clone()).await;
    let mut slot = state.lock();
    if slot
        .as_ref()
        .is_some_and(|p| Arc::ptr_eq(&p.cancel, &cancel))
    {
        *slot = None;
    }
    out
}

/// Close the listener and end its [`sso_wait`].
#[tauri::command]
pub fn sso_cancel(state: tauri::State<'_, Sso>) {
    if let Some(p) = state.lock().take() {
        p.cancel.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn send(port: u16, request: &str) -> String {
        let mut s = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        s.write_all(request.as_bytes()).await.unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).await.unwrap();
        out
    }

    #[tokio::test]
    async fn other_paths_and_methods_are_refused_and_the_redirect_still_lands() {
        let bound = bind(0..=0).unwrap();
        let port = bound.port;
        let wait = tokio::spawn(accept_redirect(
            bound,
            Duration::from_secs(10),
            Arc::new(Notify::new()),
        ));
        let favicon = send(port, "GET /favicon.ico HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
        assert!(favicon.starts_with("HTTP/1.1 404"), "{favicon}");
        let post = send(
            port,
            "POST /?code=c&state=s HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await;
        assert!(post.starts_with("HTTP/1.1 405"), "{post}");
        let bare = send(port, "GET /?code=c HTTP/1.1\r\nHost: localhost\r\n\r\n").await;
        assert!(bare.starts_with("HTTP/1.1 400"), "{bare}");
        assert!(!wait.is_finished());
        let ok = send(
            port,
            "GET /?code=abc&state=xyz&iss=x HTTP/1.1\r\nHost: localhost\r\n\r\n",
        )
        .await;
        assert!(ok.starts_with("HTTP/1.1 200"), "{ok}");
        assert!(ok.contains(r#"data-state="ok""#), "{ok}");
        assert!(ok.contains("close this tab"));
        assert_eq!(
            wait.await.unwrap().unwrap(),
            format!("http://localhost:{port}/?code=abc&state=xyz&iss=x")
        );
    }

    #[tokio::test]
    async fn an_error_redirect_is_handed_over_with_the_error_page() {
        let bound = bind(0..=0).unwrap();
        let port = bound.port;
        let wait = tokio::spawn(accept_redirect(
            bound,
            Duration::from_secs(10),
            Arc::new(Notify::new()),
        ));
        let page = send(port, "GET /?error=access_denied&state=s HTTP/1.1\r\n\r\n").await;
        assert!(page.contains(r#"data-state="err""#), "{page}");
        assert!(page.contains("Login failed"));
        assert!(
            wait.await
                .unwrap()
                .unwrap()
                .ends_with("/?error=access_denied&state=s")
        );
    }

    #[tokio::test]
    async fn the_wait_times_out_and_cancels() {
        let bound = bind(0..=0).unwrap();
        let out = accept_redirect(bound, Duration::from_millis(50), Arc::new(Notify::new())).await;
        assert!(out.unwrap_err().starts_with("no answer from the browser"));

        let bound = bind(0..=0).unwrap();
        let port = bound.port;
        let cancel = Arc::new(Notify::new());
        cancel.notify_one();
        let out = accept_redirect(bound, Duration::from_secs(10), cancel).await;
        assert_eq!(out.unwrap_err(), "SSO login cancelled");
        // Both cases dropped the sockets: the port is free again.
        assert!(std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, port)).is_ok());
    }

    #[tokio::test]
    async fn a_taken_port_moves_on_to_the_next() {
        let taken = std::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let p = taken.local_addr().unwrap().port();
        // The taken port is skipped; the range's other port may itself be busy on a shared machine.
        match bind(p..=p.saturating_add(1)) {
            Ok(b) => assert_eq!(b.port, p + 1),
            Err(e) => assert_eq!(e.kind(), io::ErrorKind::AddrInUse),
        }
        assert_eq!(
            bind(p..=p).err().map(|e| e.kind()),
            Some(io::ErrorKind::AddrInUse)
        );
    }
}
