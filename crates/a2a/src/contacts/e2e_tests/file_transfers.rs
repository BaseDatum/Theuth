//! 199 Z1: contact files across every seam on loopback. Two live engines (agent + a2a, each
//! in its own runtime, its store's engine id its iroh endpoint as in a binary), each with its
//! own file service: the sender's offers behind A's file route, the receiver's queue calling
//! `ContactFetch`, publication, receipts and notices. One process has no front door per
//! engine, so each engine's notice door is the owner's own admission (`AdmitNotice`); the
//! front door's routing is `rpc-compose`'s test.

use std::os::unix::fs::PermissionsExt as _;
use std::sync::atomic::{AtomicUsize, Ordering};

use agent::communication::files::{
    self as cf, FileState, InState, NoticeAdmission, NoticeStatus, OfferError, OfferHandle,
    OutState, Receipt,
    notices::{AdmitNotice, NoticeDoor},
    service::FileService,
};
use agent::{HistoryEntry, LoadSession, ShareMode};
use kameo::actor::ActorRef;
use runtime::SessionId;

use super::*;

/// The owner side of a notice on that engine (what the front door reaches when the chat is
/// here).
struct Door(ActorRef<agent::Agents>);

#[async_trait::async_trait]
impl NoticeDoor for Door {
    async fn notice(
        &self,
        session: SessionId,
        id: uuid::Uuid,
        contact: &str,
        text: &str,
    ) -> Result<NoticeAdmission, String> {
        self.0
            .ask(AdmitNotice {
                session,
                id,
                contact: contact.into(),
                text: text.into(),
            })
            .await
            .map_err(|e| e.to_string())
    }

    async fn status(&self, session: SessionId, id: uuid::Uuid) -> Result<NoticeStatus, String> {
        let store = self.0.ask(agent::DurableStore).await.map_err(|e| e.to_string())?;
        match store.notice_committed(session, id).await {
            Ok(true) => Ok(NoticeStatus::Committed),
            Ok(false) => Ok(NoticeStatus::Unknown),
            Err(e) => Err(format!("{e:#}")),
        }
    }
}

/// `e`'s file service, as `supervisor::start` starts it (offers registered in its runtime),
/// with a short backoff; cancel the token to stop it.
async fn service(e: &Engine) -> (Arc<FileService>, tokio_util::sync::CancellationToken) {
    let agents = e.ctx.agents().unwrap();
    let store = agents.ask(agent::DurableStore).await.unwrap();
    let mut svc = FileService::new(
        store,
        e.ctx.rt.data_dir.clone(),
        Some(e.ctx.rt.clone()),
        Arc::new(Door(agents)),
    );
    Arc::get_mut(&mut svc).unwrap().backoff = Duration::from_millis(50);
    let stop = tokio_util::sync::CancellationToken::new();
    svc.start(stop.clone());
    (svc, stop)
}

/// What A's route serves through: the engine's real offers, except that the next open of a
/// file whose length is listed in `cuts` serves only its first `n` bytes from a pipe, then
/// ends (a dropped stream: the receiver resumes); a `hold` keeps that pipe open until it is
/// released.
struct Cutting {
    inner: Arc<dyn cf::FileOffers>,
    opens: AtomicUsize,
    /// `(file length, bytes served)`, each used once.
    cuts: parking_lot::Mutex<Vec<(u64, u64)>>,
    hold: parking_lot::Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    /// The receiver's partial sizes seen at each open (its `files/partial/` files).
    seen: parking_lot::Mutex<Vec<Vec<u64>>>,
    partials: std::path::PathBuf,
}

impl Cutting {
    /// Wrap `sender`'s registered offers; `receiver`'s partials are watched.
    fn on(sender: &Engine, receiver: &Engine) -> Arc<Self> {
        let c = Arc::new(Self {
            inner: cf::offers(&sender.ctx.rt),
            opens: AtomicUsize::new(0),
            cuts: parking_lot::Mutex::default(),
            hold: parking_lot::Mutex::default(),
            seen: parking_lot::Mutex::default(),
            partials: receiver.ctx.rt.data_dir.join("files/partial"),
        });
        cf::set_offers(&sender.ctx.rt, c.clone());
        c
    }

    fn sizes(&self) -> Vec<u64> {
        std::fs::read_dir(&self.partials)
            .map(|d| {
                d.filter_map(|e| e.ok()?.metadata().ok().map(|m| m.len()))
                    .collect()
            })
            .unwrap_or_default()
    }
}

#[async_trait::async_trait]
impl cf::FileOffers for Cutting {
    async fn open(&self, contact: &str, id: &str) -> Result<OfferHandle, OfferError> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        self.seen.lock().push(self.sizes());
        let handle = self.inner.open(contact, id).await?;
        let cut = {
            let mut cuts = self.cuts.lock();
            let at = cuts.iter().position(|(len, _)| *len == handle.len);
            at.map(|i| cuts.remove(i).1)
        };
        let Some(cut) = cut else {
            return Ok(handle);
        };
        use std::io::Write as _;
        use std::os::unix::fs::FileExt as _;
        let mut head = vec![0; cut as usize];
        handle.file.read_exact_at(&mut head, 0).unwrap();
        let (reader, mut writer) = std::io::pipe().unwrap();
        let hold = self.hold.lock().take();
        std::thread::spawn(move || {
            let _ = writer.write_all(&head);
            if let Some(hold) = hold {
                let _ = hold.recv();
            }
        });
        Ok(OfferHandle {
            file: std::os::fd::OwnedFd::from(reader).into(),
            ..handle
        })
    }

    async fn receipt(&self, contact: &str, id: &str, receipt: Receipt) -> Result<(), OfferError> {
        self.inner.receipt(contact, id, receipt).await
    }
}

/// Two live engines paired as contacts, A's chat `proj` shared with B in full, both with
/// their file service and each other's card read (both take files).
struct Shared {
    a: Engine,
    b: Engine,
    /// A's name for B, B's for A.
    a_alias: String,
    b_alias: String,
    /// A's shared chat; B's originating session.
    chat: SessionId,
    origin: SessionId,
    services: Vec<(Arc<FileService>, tokio_util::sync::CancellationToken)>,
    dir: tempfile::TempDir,
}

async fn shared() -> Shared {
    let lan = Lan::default();
    let a = engine_as("a", &lan, true, cfg(600), true).await;
    let b = engine_as("b", &lan, true, cfg(600), true).await;
    let (a_alias, b_alias) = paired(&a, &b).await;
    let (a_agents, b_agents) = (a.ctx.agents().unwrap(), b.ctx.agents().unwrap());
    let chat_of = |agents: ActorRef<agent::Agents>| async move {
        agents
            .ask(agent::CreateSession {
                options: agent::CreateSessionOptions::default(),
                peer: None,
            })
            .await
            .unwrap()
            .session
    };
    let chat = chat_of(a_agents).await;
    let origin = chat_of(b_agents).await;
    a.ctx
        .put_export(Export {
            id: None,
            name: "proj".into(),
            target: ExportTarget::Chat { session: chat },
            description: "The project chat".into(),
            paused: None,
        })
        .await
        .unwrap();
    a.ctx
        .update_contact(ContactUpdate {
            name: a_alias.clone(),
            patch: ContactPatch {
                grant: Some(vec!["proj".into()]),
                ..ContactPatch::default()
            },
        })
        .await
        .unwrap();
    a.ctx.refresh_offers(&b.eid()).await;
    b.ctx.refresh_offers(&a.eid()).await;
    until("B sees proj shared in full", || {
        b.ctx
            .offers_of(&a.eid())
            .iter()
            .find(|o| o.name == "proj" && o.mode == ShareMode::Full)
            .map(|_| ())
    })
    .await;
    let takes = |e: &Engine, other: &Engine| {
        e.ctx
            .offers
            .lock()
            .get(&other.eid())
            .is_some_and(|c| c.files)
            .then_some(())
    };
    until("A read B's card (files)", || takes(&a, &b)).await;
    until("B read A's card (files)", || takes(&b, &a)).await;
    let services = vec![service(&a).await, service(&b).await];
    Shared {
        a,
        b,
        a_alias,
        b_alias,
        chat,
        origin,
        services,
        dir: tempfile::tempdir().unwrap(),
    }
}

/// `bytes` at `name` in the test's scratch dir.
fn source(s: &Shared, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let p = s.dir.path().join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

/// A tiny PNG-looking image and a PDF-looking document, each carrying a marker that must
/// never reach a transcript.
fn image_and_pdf(s: &Shared, tag: &str) -> Vec<std::path::PathBuf> {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    png.extend(format!("Z1-PIXELS-{tag}").bytes().cycle().take(40_000));
    let mut pdf = b"%PDF-1.4\n".to_vec();
    pdf.extend(format!("Z1-PAGES-{tag}").bytes().cycle().take(90_000));
    vec![
        source(s, &format!("shot-{tag}.png"), &png),
        source(s, &format!("notes-{tag}.pdf"), &pdf),
    ]
}

/// Stage `files` from `from` to `to` (the tool's path: gate, clone, hash, offers), deliver
/// the text, and settle the staging, as `send_message(files)` and `answer_peer_request` do.
async fn send(
    agents: &ActorRef<agent::Agents>,
    from: SessionId,
    to: &str,
    text: &str,
    files: &[std::path::PathBuf],
    reply_to: Option<uuid::Uuid>,
) -> Result<agent::supervisor::Delivered, String> {
    let staged = match files.is_empty() {
        true => None,
        false => Some(cf::send::stage(agents, from, &[to.to_owned()], files, None, text).await?),
    };
    let sent = agents
        .ask(agent::supervisor::Deliver {
            from,
            to: to.into(),
            text: text.into(),
            await_reply: false,
            reply_to,
            request_id: staged.as_ref().map(|s| s.request_ids[0].clone()),
            admitted: tokio_util::sync::CancellationToken::new(),
        })
        .await
        .map_err(|e| e.to_string());
    if let Some(staged) = staged {
        staged.finish().await;
    }
    sent
}

/// `session`'s user rows as stored.
async fn rows(agents: &ActorRef<agent::Agents>, session: SessionId) -> Vec<String> {
    agents
        .ask(LoadSession {
            session,
            before: None,
            limit: None,
            settled_only: false,
        })
        .await
        .map(|l| {
            l.history
                .into_iter()
                .filter_map(|h| match h {
                    HistoryEntry::User { text, .. } => Some(text),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The incoming files of `exchange` on `e`, once each is `state` (else the rows as last
/// seen).
async fn incoming(e: &Engine, exchange: uuid::Uuid, n: usize, state: &str) -> Vec<cf::ContactFile> {
    let store = e.ctx.agents().unwrap().ask(agent::DurableStore).await.unwrap();
    let ex = exchange.to_string();
    let mut last = Vec::new();
    for _ in 0..800 {
        last = store.exchange_files(&ex).await.unwrap();
        if last.len() == n && last.iter().all(|r| r.state == state) {
            return store.exchange_cards(&ex).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let seen: Vec<_> = last.iter().map(|r| (&r.name, &r.state, &r.error, &r.notice_state)).collect();
    panic!("timed out waiting for {n} file(s) {state}: {seen:?}");
}

/// The newest record of `session` whose text is `text`.
async fn record(
    agents: &ActorRef<agent::Agents>,
    session: SessionId,
    text: &'static str,
) -> agent::PeerRecord {
    eventually(text, || {
        let agents = agents.clone();
        async move {
            agents
                .ask(agent::ListPeerMessages {
                    session,
                    cursor: None,
                    limit: None,
                    pending_only: false,
                })
                .await
                .ok()?
                .messages
                .into_iter()
                .find(|r| r.text == text && r.receipt.to == format!("session:{session}"))
        }
    })
    .await
}

/// Every received file is at its path, `0600`, its bytes the source's; the chat holds the
/// text's row with one receiving placeholder per file, then exactly one notice per file,
/// and no file's bytes anywhere in its rows.
async fn check_received(
    e: &Engine,
    session: SessionId,
    text: &str,
    got: &[cf::ContactFile],
    sources: &[std::path::PathBuf],
    markers: &[&str],
) {
    let agents = e.ctx.agents().unwrap();
    for (f, src) in got.iter().zip(sources) {
        let path = f.path.clone().expect("a received file has its path");
        assert_eq!(std::fs::read(&path).unwrap(), std::fs::read(src).unwrap());
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "{}", path.display());
        assert_eq!(f.state, FileState::In(InState::Received));
    }
    let notice = |r: &str, f: &cf::ContactFile| {
        r.starts_with("<message_file ")
            && r.contains(&format!(" file=\"{}\"", f.id))
            && r.contains("state=\"received\"")
    };
    let all = eventually("one notice per file", || {
        let agents = agents.clone();
        async move {
            let all = rows(&agents, session).await;
            got.iter()
                .all(|f| all.iter().any(|r| notice(r, f)))
                .then_some(all)
        }
    })
    .await;
    // Nothing follows: a re-sent notice is never a second row.
    tokio::time::sleep(Duration::from_millis(600)).await;
    let all = {
        let later = rows(&agents, session).await;
        assert_eq!(later.len(), all.len(), "{later:#?}");
        later
    };
    for f in got {
        assert_eq!(all.iter().filter(|r| notice(r, f)).count(), 1, "{all:#?}");
    }
    let message = all
        .iter()
        .find(|r| r.contains(text) && r.starts_with("<message "))
        .unwrap_or_else(|| panic!("the text's row: {all:#?}"));
    for f in got {
        assert!(
            message.contains(&format!(" file=\"{}\"", f.id))
                && message.contains("state=\"receiving\""),
            "the text arrives with its receiving placeholders: {message}"
        );
    }
    for m in markers {
        assert!(
            !all.iter().any(|r| r.contains(m)),
            "{m}: no file bytes in the transcript"
        );
    }
}

/// D1, D2, D5, D17 across the seams: an image and a PDF B→A into A's shared chat, then A→B
/// over `~return` with `reply_to`. The text arrives with receiving placeholders, each file
/// lands verified and `0600`, one notice per file follows, no file byte reaches a
/// transcript, the sender's record lists its offers (`peer_message`) and they read `sent`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_image_and_a_pdf_go_both_ways_text_first_then_one_notice_each() {
    let s = shared().await;
    let (a_agents, b_agents) = (s.a.ctx.agents().unwrap(), s.b.ctx.agents().unwrap());

    let to_a = image_and_pdf(&s, "to-a");
    let sent = send(
        &b_agents,
        s.origin,
        &format!("peer:{}/proj", s.b_alias),
        "two for you",
        &to_a,
        None,
    )
    .await
    .unwrap();
    let x = record(&a_agents, s.chat, "two for you").await;
    let got = incoming(&s.a, x.receipt.id, 2, "received").await;
    assert_eq!(
        got.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
        ["shot-to-a.png", "notes-to-a.pdf"]
    );
    assert_eq!(got[0].media_type, "image/png");
    assert_eq!(got[1].media_type, "application/pdf");
    // A received file's holder is the engine that stored it, by its endpoint id (the id
    // `mesh_status().self_id` reads; the desktop's "is this the holder").
    assert_eq!(got[0].holder, s.a.eid(), "the holder is A's endpoint");
    check_received(&s.a, s.chat, "two for you", &got, &to_a, &["Z1-PIXELS", "Z1-PAGES"]).await;
    // B's record of its send lists the offers (the desktop's SendCard), and they read sent.
    let offered = eventually("B's offers sent", || {
        let b_agents = b_agents.clone();
        async move {
            let r = b_agents
                .ask(agent::GetPeerMessage {
                    session: s.origin,
                    id: sent.receipt.id,
                })
                .await
                .ok()?;
            (r.files.len() == 2
                && r.files.iter().all(|f| f.state == FileState::Out(OutState::Sent)))
            .then_some(r.files)
        }
    })
    .await;
    assert_eq!(offered[0].exchange, sent.receipt.id);

    // A answers with files over B's return route.
    let to_b = image_and_pdf(&s, "to-b");
    send(
        &a_agents,
        s.chat,
        &format!("peer:{}", s.a_alias),
        "and two back",
        &to_b,
        Some(x.receipt.id),
    )
    .await
    .unwrap();
    let y = record(&b_agents, s.origin, "and two back").await;
    assert_eq!(y.receipt.reply_to, Some(sent.receipt.id), "it answers B's own message");
    let back = incoming(&s.b, y.receipt.id, 2, "received").await;
    assert_eq!(back[0].holder, s.b.eid());
    check_received(&s.b, s.origin, "and two back", &back, &to_b, &["Z1-PIXELS", "Z1-PAGES"])
        .await;
    for (_, stop) in &s.services {
        stop.cancel();
    }
}

/// D2, D8: a stream cut mid-file resumes from the partial's bytes (the next open finds them
/// there and the file verifies whole); B restarted mid-download (stop, the startup recovery,
/// a new service) empties `files/partial/`, starts over from byte 0 and gives one notice.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_cut_resumes_with_range_and_a_restart_starts_over_once() {
    let mut s = shared().await;
    let (a_agents, b_agents) = (s.a.ctx.agents().unwrap(), s.b.ctx.agents().unwrap());
    let cutting = Cutting::on(&s.a, &s.b);
    let big: Vec<u8> = (0..2_000_000u32).map(|i| (i % 251) as u8).collect();

    // The cut: the first open serves 300,000 bytes and ends.
    cutting.cuts.lock().push((2_000_000, 300_000));
    let first = vec![source(&s, "first.bin", &big)];
    // B writes first so A's chat has a message to answer over the return route.
    send(&b_agents, s.origin, &format!("peer:{}/proj", s.b_alias), "hello", &[], None)
        .await
        .unwrap();
    let x = record(&a_agents, s.chat, "hello").await;
    send(&a_agents, s.chat, &format!("peer:{}", s.a_alias), "big one", &first, Some(x.receipt.id))
        .await
        .unwrap();
    let y = record(&b_agents, s.origin, "big one").await;
    let got = incoming(&s.b, y.receipt.id, 1, "received").await;
    assert_eq!(cutting.opens.load(Ordering::SeqCst), 2, "one cut, one resume");
    assert_eq!(
        cutting.seen.lock()[1],
        [300_000],
        "the resume finds the cut's bytes in the partial (Range: bytes=300000-)"
    );
    check_received(&s.b, s.origin, "big one", &got, &first, &[]).await;

    // The restart: the next open serves 500,000 bytes and holds the stream open.
    let (release, hold) = std::sync::mpsc::channel();
    *cutting.hold.lock() = Some(hold);
    cutting.cuts.lock().push((2_000_000, 500_000));
    let second = vec![source(&s, "second.bin", &big)];
    send(&a_agents, s.chat, &format!("peer:{}", s.a_alias), "another", &second, Some(x.receipt.id))
        .await
        .unwrap();
    let z = record(&b_agents, s.origin, "another").await;
    eventually("B has 500,000 bytes of it", || {
        let sizes = cutting.sizes();
        async move { (sizes == [500_000]).then_some(()) }
    })
    .await;
    let (_, stop) = s.services.pop().unwrap();
    stop.cancel();
    drop(release);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let store = b_agents.ask(agent::DurableStore).await.unwrap();
    cf::service::recover(&store, &s.b.ctx.rt.data_dir).await;
    assert!(cutting.sizes().is_empty(), "the restart empties files/partial/");
    s.services.push(service(&s.b).await);
    let again = incoming(&s.b, z.receipt.id, 1, "received").await;
    assert_eq!(cutting.opens.load(Ordering::SeqCst), 4);
    assert_eq!(cutting.seen.lock()[3], [0], "it starts over from byte 0 (a fresh partial)");
    check_received(&s.b, s.origin, "another", &again, &second, &[]).await;
    for (_, stop) in &s.services {
        stop.cancel();
    }
}

/// D10, D13: a protected file (by path, and a hard link to it outside the data dir) and a
/// 5,000,000,001-byte sparse file are refused before anything is offered or sent; a card
/// without the extension refuses files with the design's text, and the text still goes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn protected_oversize_and_extensionless_sends_are_refused_before_the_network() {
    let s = shared().await;
    let b_agents = s.b.ctx.agents().unwrap();
    // The refusal reaches B's exchange as the outbound outcome its supervisor applies.
    let _supervisor = supervise(&b_agents).await;
    let to = format!("peer:{}/proj", s.b_alias);
    let identity = s.b.ctx.rt.data_dir.join("mesh/identity");
    std::fs::create_dir_all(identity.parent().unwrap()).unwrap();
    std::fs::write(&identity, b"secret key").unwrap();
    let hard = s.dir.path().join("innocent.txt");
    std::fs::hard_link(&identity, &hard).unwrap();
    let sparse = s.dir.path().join("huge.bin");
    std::fs::File::create(&sparse)
        .unwrap()
        .set_len(5_000_000_001)
        .unwrap();
    let store = b_agents.ask(agent::DurableStore).await.unwrap();
    for (path, why) in [
        (&identity, "is theuth's own secret state and is never sent"),
        (&hard, "is theuth's own secret state and is never sent"),
        (&sparse, "5000000001 bytes (over the 5 GB limit per file)"),
    ] {
        let err = send(&b_agents, s.origin, &to, "nope", std::slice::from_ref(path), None)
            .await
            .unwrap_err();
        assert!(err.starts_with("files: ") && err.contains(why), "{err}");
    }
    assert!(
        store.files_in_state("queued").await.unwrap().is_empty(),
        "nothing offered"
    );
    assert!(
        rows(&s.a.ctx.agents().unwrap(), s.chat).await.is_empty(),
        "nothing sent"
    );

    // A's card stops advertising files (an old engine): B's client refuses before sending.
    if let Some(c) = s.b.ctx.offers.lock().get_mut(&s.a.eid()) {
        c.files = false;
    }
    let ok = source(&s, "ok.txt", b"fine");
    let sent = send(&b_agents, s.origin, &to, "with a file", &[ok], None)
        .await
        .unwrap();
    // What the tool's first answer (and the SendCard) reads: the receipt fails with the
    // client's text, and so does the offer.
    let want = "cannot receive files: their theuth does not support them yet";
    let mut r = None;
    for _ in 0..400 {
        let got = b_agents
            .ask(agent::GetPeerMessage {
                session: s.origin,
                id: sent.receipt.id,
            })
            .await
            .unwrap();
        let done = got.receipt.state == agent::PeerMessageState::Failed
            && got.files.iter().all(|f| f.state == FileState::Out(OutState::Failed));
        r = Some(got);
        if done {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let r = r.unwrap();
    assert_eq!(r.receipt.state, agent::PeerMessageState::Failed, "{r:?}");
    assert!(r.receipt.error.as_deref().unwrap_or_default().contains(want), "{:?}", r.receipt);
    assert_eq!(r.files.len(), 1);
    assert!(r.files[0].error.as_deref().unwrap_or_default().contains(want), "{:?}", r.files);
    for (_, stop) in &s.services {
        stop.cancel();
    }
}

/// D17 (States): the exchange fails `HOLD_MOVED` after one of its two files was saved and
/// before its text was read: the saved file is rolled back by inode, no notice is written,
/// and the sender's offers read failed with the reason (its receipts).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_hold_moved_exchange_rolls_back_its_saved_file_and_fails_both_offers() {
    let s = shared().await;
    let (a_agents, b_agents) = (s.a.ctx.agents().unwrap(), s.b.ctx.agents().unwrap());
    let cutting = Cutting::on(&s.b, &s.a);
    // A's chat is paused: what reaches it stays accepted (unread), its files still arrive.
    a_agents
        .ask(agent::supervisor::pause::PauseTree { desk: s.chat })
        .await
        .unwrap();
    let (release, hold) = std::sync::mpsc::channel();
    *cutting.hold.lock() = Some(hold);
    let files = vec![
        source(&s, "small.txt", b"small and quick"),
        source(&s, "slow.bin", &[7; 300_000]),
    ];
    // The slow one serves 1,000 bytes and holds; the small one is saved meanwhile.
    cutting.cuts.lock().push((300_000, 1_000));
    let sent = send(
        &b_agents,
        s.origin,
        &format!("peer:{}/proj", s.b_alias),
        "two, one slow",
        &files,
        None,
    )
    .await
    .unwrap();
    let x = record(&a_agents, s.chat, "two, one slow").await;
    let store = a_agents.ask(agent::DurableStore).await.unwrap();
    let ex = x.receipt.id.to_string();
    let saved = eventually("one file on disk, the other receiving", || {
        let store = store.clone();
        let ex = ex.clone();
        async move {
            let rows = store.exchange_files(&ex).await.ok()?;
            let done = rows.iter().find(|r| r.state == "saved")?;
            rows.iter().any(|r| r.state == "receiving").then(|| done.clone())
        }
    })
    .await;
    store
        .fail_exchange(x.receipt.id, agent::communication::network::HOLD_MOVED)
        .await
        .unwrap();
    drop(release);
    let failed = incoming(&s.a, x.receipt.id, 2, "failed").await;
    assert!(
        !std::path::Path::new(saved.path.as_deref().unwrap()).exists(),
        "the saved file is rolled back"
    );
    assert!(failed.iter().all(|f| f.path.is_none() || !f.path.as_ref().unwrap().exists()));
    let all = rows(&a_agents, s.chat).await;
    assert!(!all.iter().any(|r| r.starts_with("<message_file ")), "no notice: {all:#?}");
    let reasons = eventually("B's offers failed by the receipts", || {
        let b_agents = b_agents.clone();
        async move {
            let r = b_agents
                .ask(agent::GetPeerMessage {
                    session: s.origin,
                    id: sent.receipt.id,
                })
                .await
                .ok()?;
            r.files
                .iter()
                .all(|f| f.state == FileState::Out(OutState::Failed))
                .then(|| r.files.iter().map(|f| f.error.clone()).collect::<Vec<_>>())
        }
    })
    .await;
    assert!(
        reasons
            .iter()
            .all(|r| r.as_deref() == Some(agent::communication::network::HOLD_MOVED)),
        "{reasons:?}"
    );
    for (_, stop) in &s.services {
        stop.cancel();
    }
}
