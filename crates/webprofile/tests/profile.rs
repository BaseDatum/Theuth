//! 209 §10 P1 tests of the profile store: seal/open and wrong-aad refusal, the per-cookie delta
//! merge with two writers, the caps and `stale-epoch` (the scrub is `tests/scrub.rs`).

use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use webprofile::{
    Cookie, CookieChange, OriginStorage, Profile, ProfileError, Push, SiteJar, caps, store,
};

const SECRET: &str = "s3cr3t-session-value-0123456789";

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

#[tokio::test]
async fn seal_opens_and_a_wrong_aad_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let (p, vault) = profile(dir.path()).await;
    let mut jar = SiteJar::new("example.com");
    jar.cookies
        .push(cookie("__Host-sid", SECRET, "app.example.com"));
    let id = p.id_of("example.com");
    assert_eq!(id.len(), 32);
    let (sealed, bytes) = p.seal(&id, &jar);
    assert_eq!(bytes, serde_json::to_vec(&jar).unwrap().len());
    assert_eq!(p.unseal(&id, &sealed), Some(jar.clone()));
    // Another row's id as aad, a bare id without the table prefix, a flipped byte: all refused.
    assert_eq!(p.unseal(&p.id_of("example.org"), &sealed), None);
    let raw = URL_SAFE_NO_PAD.decode(&sealed).unwrap();
    assert!(vault.open_with_mesh_key(id.as_bytes(), &raw).is_none());
    assert!(
        vault
            .open_with_mesh_key(format!("web_sites\n{id}").as_bytes(), &raw)
            .is_some()
    );
    let mut bad = raw.clone();
    let last = bad.len() - 1;
    bad[last] ^= 1;
    assert_eq!(p.unseal(&id, &URL_SAFE_NO_PAD.encode(bad)), None);
    // A row the push wrote opens the same way and is the site's salted id, not its name.
    p.push(push(
        "tile:iPad",
        0,
        vec![set(cookie("__Host-sid", SECRET, "app.example.com"))],
    ))
    .await
    .unwrap();
    let (rid, sealed): (String, String) =
        sqlx::query_as("SELECT id, sealed FROM web_sites WHERE id != 'epoch'")
            .fetch_one(p.pool())
            .await
            .unwrap();
    assert_eq!(rid, id);
    assert_eq!(p.unseal(&rid, &sealed).unwrap().site, "example.com");
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
            set(cookie("pref", "dark", ".example.com")),
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
    // The tile signs out of `pref` (a domain cookie, stored with or without the dot) and an
    // expired set removes `cart`.
    let mut gone = cookie("cart", "", "shop.example.com");
    gone.expires = Some(1);
    p.push(push(
        "tile:iPad",
        0,
        vec![
            CookieChange::Remove {
                name: "pref".into(),
                domain: "example.com".into(),
                path: "/".into(),
            },
            set(gone),
        ],
    ))
    .await
    .unwrap();
    let site = &p
        .load(Some(&["app.example.com".to_owned()]))
        .await
        .unwrap()
        .sites[0];
    assert_eq!(names(&site.jar), ["__Host-sid"]);
    // Two sites in one push are two rows; the Settings list counts, never values.
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
    // An older capture of the same origin is ignored; a newer one replaces it.
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

#[tokio::test]
async fn a_push_under_an_older_epoch_is_stale() {
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
    // Sign out everywhere elsewhere: the epoch moves on, rows of the old epoch are dead.
    p.set_epoch(1).await.unwrap();
    assert!(p.load(None).await.unwrap().sites.is_empty());
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
    // A row over 128 KiB (200 cookies of 1 KiB) is refused too.
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
    // Removing every cookie of a site forgets the row (sealed NULL), which frees a slot.
    p.push(push(
        "tile:iPad",
        0,
        vec![
            CookieChange::Remove {
                name: "ok".into(),
                domain: "other.org".into(),
                path: "/".into(),
            },
            CookieChange::Remove {
                name: "t".into(),
                domain: "other.org".into(),
                path: "/".into(),
            },
        ],
    ))
    .await
    .unwrap();
    let (n,): (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM web_sites WHERE sealed IS NULL AND id != 'epoch'")
            .fetch_one(p.pool())
            .await
            .unwrap();
    assert_eq!(n, 1);
    p.push(push(
        "tile:iPad",
        0,
        vec![set(cookie("s", "v", "one-more.com"))],
    ))
    .await
    .unwrap();
}
