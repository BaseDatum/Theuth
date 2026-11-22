//! 209 §9, G3: cookie and storage values never reach `Debug`, log lines, summaries, events or the
//! file at rest. Its own test binary: the subscriber is the process-wide default, so no other
//! test's thread can leave a callsite's interest cached as "off".

use std::sync::Arc;

use webprofile::{Cookie, CookieChange, OriginStorage, Profile, Push, caps, store};

const SECRET: &str = "s3cr3t-session-value-0123456789";

fn cookie(name: &str, value: &str, domain: &str) -> Cookie {
    Cookie {
        name: name.into(),
        value: value.into(),
        domain: domain.into(),
        host_only: name.starts_with("__Host-"),
        path: "/".into(),
        expires: Some(4_000_000_000),
        secure: true,
        http_only: true,
        same_site: "Lax".into(),
        partitioned: false,
    }
}

fn push(changes: Vec<CookieChange>) -> Push {
    Push {
        writer: "tile:iPad".into(),
        epoch: 0,
        changes,
        storage: vec![],
    }
}

/// A `Write` sink the subscriber logs into.
#[derive(Clone, Default)]
struct Sink(Arc<parking_lot::Mutex<Vec<u8>>>);

impl std::io::Write for Sink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn values_never_reach_debug_logs_summaries_or_the_file() {
    let sink = Sink::default();
    let writer = sink.clone();
    tracing::subscriber::set_global_default(
        tracing_subscriber::fmt()
            .with_max_level(tracing::Level::TRACE)
            .with_writer(move || writer.clone())
            .finish(),
    )
    .unwrap();
    let dir = tempfile::tempdir().unwrap();
    let vault = Arc::new(
        secrets::Vault::open(dir.path(), secrets::KeyStore::File)
            .await
            .unwrap(),
    );
    let p = Profile::open(&dir.path().join(store::DB), vault)
        .await
        .unwrap();
    let mut msg = push(vec![CookieChange::Set {
        cookie: cookie("__Host-sid", SECRET, "app.example.com"),
    }]);
    msg.storage = vec![OriginStorage {
        origin: "https://app.example.com".into(),
        local_storage: vec![("token".into(), SECRET.into())],
        captured_ms: 1,
    }];
    let debug = format!("{msg:?}");
    assert!(!debug.contains(SECRET), "{debug}");
    assert!(debug.contains("__Host-sid"));
    let r = p.push(msg).await.unwrap();
    // A refused site logs a warning: by count and site, never by value.
    let over: Vec<_> = (0..=caps::SITE_COOKIES)
        .map(|i| CookieChange::Set {
            cookie: cookie(&format!("c{i}"), SECRET, "example.com"),
        })
        .collect();
    p.push(push(over)).await.unwrap();
    let loaded = p.load(None).await.unwrap();
    assert_eq!(
        loaded.sites[0].jar.cookies[0].value, SECRET,
        "kept, only hidden"
    );
    for shown in [
        format!("{r:?}"),
        format!("{loaded:?}"),
        serde_json::to_string(&p.sites().await.unwrap()).unwrap(),
        serde_json::to_string(&webprofile::WebProfileChanged {
            sites: r.sites.clone(),
            epoch: r.epoch,
        })
        .unwrap(),
    ] {
        assert!(!shown.contains(SECRET), "{shown}");
    }
    let logs = String::from_utf8(sink.0.lock().clone()).unwrap();
    assert!(logs.contains("refused"), "{logs}");
    assert!(!logs.contains(SECRET), "{logs}");
    // At rest: neither the value nor the site name is readable in the file.
    drop(p);
    let mut bytes = Vec::new();
    for f in std::fs::read_dir(dir.path()).unwrap() {
        let f = f.unwrap().path();
        if f.file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with(store::DB)
        {
            bytes.extend(std::fs::read(&f).unwrap());
        }
    }
    assert!(!bytes.is_empty());
    let text = String::from_utf8_lossy(&bytes);
    assert!(!text.contains(SECRET));
    assert!(!text.contains("example.com"));
}
