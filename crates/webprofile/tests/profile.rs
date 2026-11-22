//! 209 §10 P1 tests of the profile store: seal/open and wrong-aad refusal, the per-cookie delta
//! merge with two writers, the caps, `stale-epoch` / `unknown-epoch`, the 30-day session-cookie
//! rule, an emptied localStorage, a mesh key rotation and rows of an unknown format version (the
//! scrub is `tests/scrub.rs`, replication `tests/replicate.rs`).

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use webprofile::{
    Cookie, CookieChange, OriginStorage, Profile, ProfileError, Push, SiteJar, caps, store,
};

const SECRET: &str = "s3cr3t-session-value-0123456789";
const DAY_MS: u64 = 24 * 3600 * 1000;

async fn profile(dir: &std::path::Path) -> (Profile, Arc<secrets::Vault>) {
    let vault = Arc::new(
        secrets::Vault::open(dir, secrets::KeyStore::File)
            .await
            .unwrap(),
    );
    let p = Profile::open(&dir.join(store::DB), vault.clone())
        .await
        .unwrap();
    (p, vault)
}

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

fn set(c: Cookie) -> CookieChange {
    CookieChange::Set { cookie: c }
}

fn remove(name: &str, domain: &str) -> CookieChange {
    CookieChange::Remove {
        name: name.into(),
        domain: domain.into(),
        path: "/".into(),
    }
}

fn push(writer: &str, epoch: u64, changes: Vec<CookieChange>) -> Push {
    Push {
        writer: writer.into(),
        epoch,
        changes,
        storage: vec![],
    }
}

fn names(jar: &SiteJar) -> Vec<&str> {
    let mut v: Vec<&str> = jar.cookies.iter().map(|c| c.name.as_str()).collect();
    v.sort_unstable();
    v
}

async fn rows(p: &Profile) -> Vec<(String, Option<String>, Option<String>)> {
    sqlx::query_as("SELECT id, site_id, sealed FROM web_sites WHERE id != 'epoch' ORDER BY id")
        .fetch_all(p.pool())
        .await
        .unwrap()
}

#[tokio::test]
async fn seal_opens_and_a_wrong_aad_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (p, vault) = profile(dir.path()).await;
    p.push(push(
        "tile:iPad",
        0,
        vec![
            set(cookie("__Host-sid", SECRET, "app.example.com")),
            set(cookie("pref", "dark", "example.com")),
        ],
    ))
    .await
    .unwrap();
    // One row per cookie, both under the site's keyed `site_id`, named by keyed hashes.
    let all = rows(&p).await;
    assert_eq!(all.len(), 2);
    let site_id = p.site_id("example.com");
    assert_eq!(site_id.len(), 32);
    for (id, sid, _) in &all {
        assert_eq!(id.len(), 32);
        assert!(id.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(sid.as_deref(), Some(site_id.as_str()));
    }
    // The row opens under its own id as aad: the entry carries site, epoch, writer and time with
    // the cookie (one atomic value). Another row's id, a bare id, a flipped byte: all refused.
    let (id, _, sealed) = &all[0];
    let raw = URL_SAFE_NO_PAD.decode(sealed.as_deref().unwrap()).unwrap();
    let plain = vault
        .open_with_mesh_key(format!("web_sites\n{id}").as_bytes(), &raw)
        .expect("opens under its own id");
    let entry: serde_json::Value = serde_json::from_slice(&plain).unwrap();
    assert_eq!(entry["v"], 1);
    assert_eq!(entry["site"], "example.com");
    assert_eq!(entry["epoch"], 0);
    assert_eq!(entry["writer"], "tile:iPad");
    assert!(entry["updated_ms"].as_u64().unwrap() > 0);
    assert!(entry["item"]["cookie"]["name"].is_string());
    assert!(vault.open_with_mesh_key(id.as_bytes(), &raw).is_none());
    let other = &all[1].0;
    assert!(
        vault
            .open_with_mesh_key(format!("web_sites\n{other}").as_bytes(), &raw)
            .is_none()
    );
    let mut bad = raw.clone();
    let last = bad.len() - 1;
    bad[last] ^= 1;
    sqlx::query("UPDATE web_sites SET sealed = ? WHERE id = ?")
        .bind(URL_SAFE_NO_PAD.encode(bad))
        .bind(id)
        .execute(p.pool())
        .await
        .unwrap();
    let load = p.load(None).await.unwrap();
    assert_eq!(
        load.sites[0].jar.cookies.len(),
        1,
        "the damaged row is skipped"
    );
}

#[tokio::test]
async fn deltas_from_two_writers_merge_per_cookie() {
    let dir = tempfile::tempdir().unwrap();
    let (p, _) = profile(dir.path()).await;
    // The tile signs in: two cookies.
    p.push(push(
        "tile:iPad",
        0,
        vec![
            set(cookie("__Host-sid", "a1", "app.example.com")),
            set(cookie("pref", "dark", "example.com")),
        ],
    ))
    .await
    .unwrap();
    // An agent browser that loaded only `__Host-sid` refreshes it and adds its own: it never saw
    // `pref` and does not delete it.
    let r = p
        .push(push(
            "obscura:engine-b",
            0,
            vec![
                set(cookie("__Host-sid", "a2", "app.example.com")),
                set(cookie("cart", "1", "shop.example.com")),
            ],
        ))
        .await
        .unwrap();
    assert_eq!(r.sites, ["example.com"]);
    assert_eq!(r.applied, 2);
    let site = &p.load(None).await.unwrap().sites[0];
    assert_eq!(names(&site.jar), ["__Host-sid", "cart", "pref"]);
    assert_eq!(
        site.jar
            .cookies
            .iter()
            .find(|c| c.name == "__Host-sid")
            .unwrap()
            .value,
        "a2"
    );
    assert_eq!(site.writer, "obscura:engine-b");
    // A set of an unchanged cookie writes nothing.
    assert!(
        p.push(push(
            "tile:Mac",
            0,
            vec![set(cookie("pref", "dark", "example.com"))]
        ))
        .await
        .unwrap()
        .sites
        .is_empty()
    );
    // The tile signs out of `pref` and an expired set removes `cart`: two tombstones.
    let mut gone = cookie("cart", "", "shop.example.com");
    gone.expires = Some(1);
    p.push(push(
        "tile:iPad",
        0,
        vec![remove("pref", "example.com"), set(gone)],
    ))
    .await
    .unwrap();
    let site = &p
        .load(Some(&["app.example.com".to_owned()]))
        .await
        .unwrap()
        .sites[0];
    assert_eq!(names(&site.jar), ["__Host-sid"]);
    let tombstones = rows(&p).await.iter().filter(|r| r.2.is_none()).count();
    assert_eq!(tombstones, 2);
    // Two sites in one push; the Settings list counts, never values.
    let mut s = OriginStorage {
        origin: "https://app.example.com".into(),
        local_storage: vec![("token".into(), SECRET.into())],
        captured_ms: 10,
    };
    let mut both = push(
        "tile:iPad",
        0,
        vec![set(cookie("sid", "x", "www.example.co.uk"))],
    );
    both.storage = vec![s.clone()];
    assert_eq!(
        p.push(both).await.unwrap().sites,
        ["example.co.uk", "example.com"]
    );
    // An older capture of the same origin is ignored.
    s.captured_ms = 5;
    s.local_storage = vec![("token".into(), "old".into())];
    let mut older = push("tile:Mac", 0, vec![]);
    older.storage = vec![s];
    assert!(p.push(older).await.unwrap().sites.is_empty());
    let list = p.sites().await.unwrap();
    let ex = list.iter().find(|s| s.site == "example.com").unwrap();
    assert_eq!((ex.cookies, ex.origins), (1, 1));
    let site = &p
        .load(Some(&["example.com".to_owned()]))
        .await
        .unwrap()
        .sites[0];
    assert_eq!(site.jar.origins[0].local_storage[0].1, SECRET);
}

/// 209 §4.2: a site that empties its localStorage is a capture: the stored snapshot does not come
/// back on the next load.
#[tokio::test]
async fn an_emptied_local_storage_replaces_the_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let (p, _) = profile(dir.path()).await;
    let snap = |captured_ms, local_storage| OriginStorage {
        origin: "https://app.example.com".into(),
        local_storage,
        captured_ms,
    };
    let mut first = push("tile:iPad", 0, vec![set(cookie("sid", "1", "example.com"))]);
    first.storage = vec![snap(1, vec![("k".into(), "v".into())])];
    p.push(first).await.unwrap();
    let mut emptied = push("obscura:B", 0, vec![]);
    emptied.storage = vec![snap(2, vec![])];
    assert_eq!(p.push(emptied).await.unwrap().sites, ["example.com"]);
    let site = &p.load(None).await.unwrap().sites[0];
    assert_eq!(site.jar.origins.len(), 1);
    assert!(site.jar.origins[0].local_storage.is_empty());
    assert_eq!(p.sites().await.unwrap()[0].origins, 0);
}

#[tokio::test]
async fn pushes_under_another_epoch_are_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (p, _) = profile(dir.path()).await;
    assert_eq!(p.epoch().await.unwrap(), 0);
    p.push(push(
        "tile:iPad",
        0,
        vec![set(cookie("sid", "1", "example.com"))],
    ))
    .await
    .unwrap();
    // A client cannot make up an epoch.
    assert_eq!(
        p.push(push("tile:iPad", 5, vec![])).await,
        Err(ProfileError::UnknownEpoch { current: 0 })
    );
    // Sign out everywhere elsewhere: the epoch moves on and every row is a tombstone.
    p.set_epoch(1).await.unwrap();
    assert!(p.load(None).await.unwrap().sites.is_empty());
    assert!(rows(&p).await.iter().all(|r| r.2.is_none()));
    assert_eq!(
        p.push(push(
            "tile:iPad",
            0,
            vec![set(cookie("sid", "2", "example.com"))]
        ))
        .await,
        Err(ProfileError::StaleEpoch { current: 1 })
    );
    assert_eq!(
        ProfileError::StaleEpoch { current: 1 }.to_string(),
        "stale-epoch"
    );
    // A writer on the new epoch starts the site afresh (the old jar is not resurrected).
    let r = p
        .push(push(
            "tile:iPad",
            1,
            vec![set(cookie("new", "1", "example.com"))],
        ))
        .await
        .unwrap();
    assert_eq!(r.epoch, 1);
    let loaded = p.load(None).await.unwrap();
    assert_eq!(loaded.epoch, 1);
    assert_eq!(names(&loaded.sites[0].jar), ["new"]);
}

/// 209 §11 Q1 (decided): a session cookie stays until its site goes 30 days without a write.
#[tokio::test]
async fn session_cookies_go_after_thirty_idle_days() {
    let dir = tempfile::tempdir().unwrap();
    let (p, _) = profile(dir.path()).await;
    let now = store::now_ms();
    let mut session = cookie("sid", "s", "example.com");
    session.expires = None;
    let persistent = cookie("keep", "k", "example.com");
    p.push_at(
        push("tile:iPad", 0, vec![set(session), set(persistent)]),
        now - 31 * DAY_MS,
    )
    .await
    .unwrap();
    assert_eq!(names(&p.load(None).await.unwrap().sites[0].jar), ["keep"]);
    // A write to the site within the window keeps it.
    p.push_at(
        push(
            "tile:iPad",
            0,
            vec![set(cookie("other", "o", "example.com"))],
        ),
        now - 29 * DAY_MS,
    )
    .await
    .unwrap();
    assert_eq!(
        names(&p.load(None).await.unwrap().sites[0].jar),
        ["keep", "other", "sid"]
    );
}

#[tokio::test]
async fn caps_refuse_without_cutting() {
    let dir = tempfile::tempdir().unwrap();
    let (p, _) = profile(dir.path()).await;
    // More than 300 cookies for one site: that site is refused, others in the push still land.
    let mut many: Vec<_> = (0..=caps::SITE_COOKIES)
        .map(|i| set(cookie(&format!("c{i}"), "v", "example.com")))
        .collect();
    many.push(set(cookie("ok", "v", "other.org")));
    let r = p.push(push("tile:iPad", 0, many)).await.unwrap();
    assert_eq!(r.sites, ["other.org"]);
    // A site over 128 KiB (200 cookies of 1 KiB) is refused too.
    let big = "x".repeat(1024);
    let heavy: Vec<_> = (0..200)
        .map(|i| set(cookie(&format!("c{i}"), &big, "heavy.com")))
        .collect();
    assert!(
        p.push(push("tile:iPad", 0, heavy))
            .await
            .unwrap()
            .sites
            .is_empty()
    );
    // A localStorage snapshot over 96 KiB is dropped whole; the cookies of the push stay.
    let mut snap = push(
        "tile:iPad",
        0,
        vec![set(cookie("sid", "v", "app.example.net"))],
    );
    snap.storage = vec![OriginStorage {
        origin: "https://app.example.net".into(),
        local_storage: vec![("k".into(), "y".repeat(caps::ORIGIN_STORAGE_BYTES))],
        captured_ms: 1,
    }];
    p.push(snap).await.unwrap();
    let site = &p
        .load(Some(&["example.net".to_owned()]))
        .await
        .unwrap()
        .sites[0];
    assert_eq!(names(&site.jar), ["sid"]);
    assert!(site.jar.origins.is_empty());
    // 500 live sites: the 501st new site is `profile full`; an existing site still updates.
    let fill: Vec<_> = (0..caps::SITES)
        .map(|i| set(cookie("s", "v", &format!("site{i}.com"))))
        .collect();
    let r = p.push(push("tile:iPad", 0, fill)).await.unwrap();
    assert_eq!(
        r.sites.len(),
        caps::SITES - 2,
        "two sites were live already"
    );
    assert_eq!(
        p.push(push(
            "tile:iPad",
            0,
            vec![set(cookie("s", "v", "one-more.com"))]
        ))
        .await,
        Err(ProfileError::Full)
    );
    assert_eq!(ProfileError::Full.to_string(), webprofile::PROFILE_FULL);
    assert_eq!(
        p.push(push(
            "tile:iPad",
            0,
            vec![set(cookie("t", "v", "other.org"))]
        ))
        .await
        .unwrap()
        .sites,
        ["other.org"]
    );
    // Removing every cookie of a site tombstones its rows, which frees a slot.
    p.push(push(
        "tile:iPad",
        0,
        vec![remove("ok", "other.org"), remove("t", "other.org")],
    ))
    .await
    .unwrap();
    let (n,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM web_sites WHERE sealed IS NULL AND site_id = ?")
            .bind(p.site_id("other.org"))
            .fetch_one(p.pool())
            .await
            .unwrap();
    assert_eq!(n, 2);
    p.push(push(
        "tile:iPad",
        0,
        vec![set(cookie("s", "v", "one-more.com"))],
    ))
    .await
    .unwrap();
}

/// 209 §4.1: ids follow the mesh key. After a rotation (here: adopting another keyring, the old key
/// retained), the rows move to the new key's ids and still load.
#[tokio::test]
async fn rows_move_to_the_ids_of_a_new_mesh_key() {
    let dir = tempfile::tempdir().unwrap();
    let (p, vault) = profile(dir.path()).await;
    p.push(push(
        "tile:iPad",
        0,
        vec![set(cookie("sid", SECRET, "example.com"))],
    ))
    .await
    .unwrap();
    let before = rows(&p).await;
    let old_site = p.site_id("example.com");
    let other_dir = tempfile::tempdir().unwrap();
    let other = secrets::Vault::open(other_dir.path(), secrets::KeyStore::File)
        .await
        .unwrap();
    vault.adopt_keyring(other.keyring()).await.unwrap();
    p.maintain().await.unwrap();
    let new_site = p.site_id("example.com");
    assert_ne!(old_site, new_site);
    let after = rows(&p).await;
    let live: Vec<_> = after.iter().filter(|r| r.2.is_some()).collect();
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].1.as_deref(), Some(new_site.as_str()));
    assert_ne!(live[0].0, before[0].0);
    assert!(
        after.iter().any(|r| r.0 == before[0].0 && r.2.is_none()),
        "old id tombstoned"
    );
    let load = p.load(Some(&["example.com".to_owned()])).await.unwrap();
    assert_eq!(load.sites[0].jar.cookies[0].value, SECRET);
}

/// A row of a format version this build does not know is skipped, not misread.
#[tokio::test]
async fn rows_of_an_unknown_version_are_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let (p, vault) = profile(dir.path()).await;
    p.push(push(
        "tile:iPad",
        0,
        vec![set(cookie("sid", "1", "example.com"))],
    ))
    .await
    .unwrap();
    let id = "0123456789abcdef0123456789abcdef";
    let future = serde_json::json!({ "v": 2, "site": "example.com", "epoch": 0, "writer": "x",
        "updated_ms": 1, "item": { "cookie": cookie("v2", "1", "example.com") } });
    let sealed = vault.seal_with_mesh_key(
        format!("web_sites\n{id}").as_bytes(),
        &serde_json::to_vec(&future).unwrap(),
    );
    sqlx::query("INSERT INTO web_sites (id, site_id, sealed) VALUES (?, ?, ?)")
        .bind(id)
        .bind(p.site_id("example.com"))
        .bind(URL_SAFE_NO_PAD.encode(sealed))
        .execute(p.pool())
        .await
        .unwrap();
    for load in [
        p.load(None).await.unwrap(),
        p.load(Some(&["example.com".to_owned()])).await.unwrap(),
    ] {
        assert_eq!(names(&load.sites[0].jar), ["sid"]);
    }
}
