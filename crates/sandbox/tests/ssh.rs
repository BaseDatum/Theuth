//! Against a real host: `SANDBOX_TEST_SSH=user@host cargo test -p sandbox -- --ignored`.
//! System transport by default; `SANDBOX_TEST_KEY_FILE=<pem path>` runs the
//! built-in one with that key instead (the vault is not involved here).

use bytes::Bytes;
use sandbox::ssh::{SshSandbox, Target};
use sandbox::{Cmd, DaemonKind, Progress, Sandbox};

#[tokio::test]
#[ignore = "needs SANDBOX_TEST_SSH=user@host"]
async fn exec_put_detect_round_trip() {
    let target = Target::parse(&std::env::var("SANDBOX_TEST_SSH").expect("SANDBOX_TEST_SSH")).unwrap();
    let sb = match std::env::var("SANDBOX_TEST_KEY_FILE") {
        Ok(p) => SshSandbox::builtin(target, &std::fs::read_to_string(p).unwrap()).unwrap(),
        Err(_) => SshSandbox::system(target),
    };
    let out = Progress::stdout();
    let h = sb.create(&out).await.unwrap();

    let caps = sb.capabilities(&h).await.unwrap();
    assert!(caps.os == "linux" || caps.os == "macos", "{caps:?}");
    assert_eq!(caps.daemon, DaemonKind::Service);

    let echo = sb.exec(&h, Cmd::new("printf hi; printf err >&2; exit 3")).await.unwrap();
    assert_eq!((echo.status, echo.stdout.as_slice(), echo.stderr.as_slice()), (3, &b"hi"[..], &b"err"[..]));

    let big = Bytes::from(vec![7u8; 3 << 20]);
    let path = format!("/tmp/sandbox-put-{}", std::process::id());
    sb.put(&h, &path, big.clone(), 0o600).await.unwrap();
    let back = sb.exec(&h, Cmd::new(format!("wc -c < {path} && stat -c %a {path} 2>/dev/null || stat -f %Lp {path}; rm -f {path}"))).await.unwrap();
    let text = back.stdout_str();
    assert!(text.starts_with(&big.len().to_string()), "{text}");
    assert!(text.ends_with("600"), "{text}");
}
