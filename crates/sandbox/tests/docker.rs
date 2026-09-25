//! Against a Docker daemon: `SANDBOX_TEST_T=/path/to/linux/t cargo test -p sandbox -- --ignored`
//! builds `Dockerfile.dev` with that binary, starts the container, exercises
//! exec/put/adopt/recreate, removes it and its volumes.

use bytes::Bytes;
use sandbox::docker::DockerSandbox;
use sandbox::{Cmd, DaemonKind, Progress, Sandbox};

#[tokio::test]
#[ignore = "needs a Docker daemon and SANDBOX_TEST_T=<linux t>"]
async fn container_round_trip() {
    let t = std::path::PathBuf::from(std::env::var("SANDBOX_TEST_T").expect("SANDBOX_TEST_T"));
    let name = format!("itest-{}", std::process::id());
    let sb = DockerSandbox::connect(&name, None, Some(t)).await.unwrap();
    let out = Progress::stdout();

    let h = sb.create(&out).await.unwrap();
    let caps = sb.capabilities(&h).await.unwrap();
    assert_eq!((caps.os.as_str(), caps.daemon), ("linux", DaemonKind::Entrypoint));

    let v = sb.exec(&h, Cmd::new("t --version")).await.unwrap();
    assert!(v.ok(), "{}", v.reason());

    sb.put(&h, "/workspace/dir/hello", Bytes::from_static(b"hello"), 0o640).await.unwrap();
    let back = sb.exec(&h, Cmd::new("cat /workspace/dir/hello; stat -c ' %a %u' /workspace/dir/hello")).await.unwrap();
    assert_eq!(back.stdout_str(), "hello 640 1000");

    // Second create adopts the running container.
    let again = sb.create(&out).await.unwrap();
    assert_eq!(again, h);

    sb.destroy(&h, false).await.unwrap();
    let gone = sb.exec(&h, Cmd::new("true")).await;
    assert!(gone.is_err());
}
