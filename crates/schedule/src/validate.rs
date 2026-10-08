//! Creation-time validation of a `when:` file (40 §1.12): resolve the sensor,
//! dry-run `check` twice, run `probe`, and say what the gate would do now.
//! The `schedule` tool refuses the file on `Err`; `/schedules test` and the
//! desktop's Test button show the `Ok` verdict through [`render`].

use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use agent::sensor::{SensorError, SensorMeta, SensorRun, SensorRunner, payload_hash};

use crate::Verdict;
use crate::file::{Fire, Spec};

/// The three sensor calls the dry run makes, so tests can run it against a
/// scripted runner; [`SensorRunner`] is the only production implementation.
pub(crate) trait Runner {
    fn resolve(&self, file: &str) -> Result<SensorMeta, SensorError>;
    async fn check(
        &self,
        meta: &SensorMeta,
        args: &BTreeMap<String, String>,
        state: &serde_json::Value,
        cwd: Option<&Path>,
        timeout: Duration,
    ) -> Result<SensorRun, SensorError>;
    async fn probe(
        &self,
        meta: &SensorMeta,
        args: &BTreeMap<String, String>,
        cwd: Option<&Path>,
        timeout: Duration,
    ) -> Result<Option<String>, SensorError>;
}

impl Runner for SensorRunner {
    fn resolve(&self, file: &str) -> Result<SensorMeta, SensorError> {
        SensorRunner::resolve(self, file)
    }

    async fn check(
        &self,
        meta: &SensorMeta,
        args: &BTreeMap<String, String>,
        state: &serde_json::Value,
        cwd: Option<&Path>,
        timeout: Duration,
    ) -> Result<SensorRun, SensorError> {
        SensorRunner::check(self, meta, args, state, cwd, timeout).await
    }

    async fn probe(
        &self,
        meta: &SensorMeta,
        args: &BTreeMap<String, String>,
        cwd: Option<&Path>,
        timeout: Duration,
    ) -> Result<Option<String>, SensorError> {
        SensorRunner::probe(self, meta, args, cwd, timeout).await
    }
}

/// The 40 §1.12 dry run: resolve (`when: no such sensor: …` / `when: needs
/// cpython, none found`), run `check` twice with an empty state under the
/// spec's `timeout` (`dry run: <reason>` on either failure; `elapsed_ms` is
/// the slower), `flaps` when the two payloads hash differently, then `probe`
/// when the sensor has one (`probe: <reason>` refuses). `last_hash` is the
/// last fired payload's hash (`None` at creation): `would_fire` is `fire`
/// unless `fire: on_change` and the hash is unchanged.
pub async fn dry_run(
    spec: &Spec,
    sensors: &SensorRunner,
    last_hash: Option<&str>,
) -> Result<Verdict, String> {
    run(spec, sensors, last_hash).await.map(|(v, _)| v)
}

/// [`dry_run`] plus the first payload's `subject=` envelope value: the title
/// `BindSession` gives the watcher's session at creation (40 §1.12).
pub async fn dry_run_with_subject(
    spec: &Spec,
    sensors: &SensorRunner,
) -> Result<(Verdict, Option<String>), String> {
    run(spec, sensors, None).await
}

/// 108 §9.4: the verdict on an unsaved file — `render` + `parse` as `Put`
/// does, then [`dry_run`] with no last hash. Nothing is written, bound or
/// reconciled. `Err` with the parse error or `not a watcher`.
pub async fn test_file(
    file: &crate::file::ScheduleFile,
    sensors: &SensorRunner,
) -> Result<Verdict, String> {
    if !crate::file::valid_name(&file.name) {
        return Err("name: [a-z0-9][a-z0-9-]*, at most 40 chars".into());
    }
    let spec = crate::file::parse(&file.name, &crate::file::render(&file.args()))?;
    dry_run(&spec, sensors, None).await
}

async fn run<R: Runner>(
    spec: &Spec,
    sensors: &R,
    last_hash: Option<&str>,
) -> Result<(Verdict, Option<String>), String> {
    let sensor = spec.sensor.as_ref().ok_or("not a watcher")?;
    let meta = sensors.resolve(&sensor.file).map_err(|e| e.to_string())?;
    let first = check_once(sensors, &meta, spec)
        .await
        .map_err(|e| format!("dry run: {e}"))?;
    let second = check_once(sensors, &meta, spec)
        .await
        .map_err(|e| format!("dry run: {e}"))?;
    let hash = payload_hash(&first.payload);
    let flaps = hash != payload_hash(&second.payload);
    let probe = if meta.has_probe {
        match sensors
            .probe(&meta, &sensor.args, spec.cwd.as_deref(), spec.timeout)
            .await
        {
            Ok(None) => Some("ok".to_owned()),
            Ok(Some(reason)) => return Err(format!("probe: {reason}")),
            Err(e) => return Err(format!("probe: {e}")),
        }
    } else {
        None
    };
    let would_fire =
        first.fire && (spec.fire != Fire::OnChange || last_hash != Some(hash.as_str()));
    let subject = agent::sensor::parse_envelope(&first.payload).remove("subject");
    let verdict = Verdict {
        sensor: meta.name,
        elapsed_ms: u64::try_from(first.elapsed.max(second.elapsed).as_millis())
            .unwrap_or(u64::MAX),
        fire: first.fire,
        hash,
        flaps,
        probe,
        would_fire,
        needs_approvals_off: spec.approvals && body_mutates(spec),
        error: None,
    };
    Ok((verdict, subject))
}

async fn check_once<R: Runner>(
    sensors: &R,
    meta: &SensorMeta,
    spec: &Spec,
) -> Result<SensorRun, SensorError> {
    let sensor = spec.sensor.as_ref().expect("checked by caller");
    sensors
        .check(
            meta,
            &sensor.args,
            &serde_json::Value::Object(Default::default()),
            spec.cwd.as_deref(),
            spec.timeout,
        )
        .await
}

/// Does the body look like it will change the world (40 §1.12: "needs
/// `approvals: false` when the body will mutate")? A conservative keyword
/// heuristic — `push`, `merge`, `commit`, `write`, `gh pr` anywhere in the
/// body, case-insensitive, substrings included (`pushed`, `written`) — so it
/// over-reports rather than lets an unattended prompt stall on an approval.
pub fn body_mutates(spec: &Spec) -> bool {
    let body = spec.prompt.to_ascii_lowercase();
    ["push", "merge", "commit", "write", "gh pr"]
        .iter()
        .any(|k| body.contains(k))
}

/// One line for the tool reply, `/schedules test` and the desktop: `sensor
/// pr_watch.py · 1.2 s · fire: true · would NOT fire now · hash a1b2c3d4… ·
/// probe ok · needs approvals: false (body mutates) · payload flaps — …`;
/// `sensor x.py · check broken: <error>` when `error` is set.
pub fn render(v: &Verdict) -> String {
    if let Some(e) = &v.error {
        return format!("sensor {} · check broken: {e}", v.sensor);
    }
    let hash = if v.hash.is_empty() {
        "—".to_owned()
    } else {
        format!("{}…", v.hash.chars().take(8).collect::<String>())
    };
    let mut s = format!(
        "sensor {} · {:.1} s · fire: {} · would {}fire now · hash {hash}",
        v.sensor,
        v.elapsed_ms as f64 / 1000.0,
        v.fire,
        if v.would_fire { "" } else { "NOT " },
    );
    if let Some(p) = &v.probe {
        s.push_str(&format!(" · probe {p}"));
    }
    if v.needs_approvals_off {
        s.push_str(" · needs approvals: false (body mutates)");
    }
    if v.flaps {
        s.push_str(" · payload flaps — include only stable fields");
    }
    s
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::collections::VecDeque;

    use super::*;
    use crate::file;

    struct Fake {
        meta: Result<SensorMeta, SensorError>,
        checks: RefCell<VecDeque<Result<SensorRun, SensorError>>>,
        probe: Result<Option<String>, SensorError>,
    }

    impl Runner for Fake {
        fn resolve(&self, _: &str) -> Result<SensorMeta, SensorError> {
            self.meta.clone()
        }
        async fn check(
            &self,
            _: &SensorMeta,
            _: &BTreeMap<String, String>,
            state: &serde_json::Value,
            _: Option<&Path>,
            timeout: Duration,
        ) -> Result<SensorRun, SensorError> {
            assert_eq!(state, &serde_json::json!({}));
            assert_eq!(timeout, Duration::from_secs(20));
            self.checks.borrow_mut().pop_front().expect("a third check")
        }
        async fn probe(
            &self,
            _: &SensorMeta,
            _: &BTreeMap<String, String>,
            _: Option<&Path>,
            _: Duration,
        ) -> Result<Option<String>, SensorError> {
            self.probe.clone()
        }
    }

    fn meta(has_probe: bool) -> SensorMeta {
        SensorMeta {
            name: "fs_exists.py".into(),
            path: "/tmp/fs_exists.py".into(),
            version: None,
            needs_cpython: false,
            hosts: Vec::new(),
            has_probe,
            args: Vec::new(),
            description: None,
            usage: None,
        }
    }

    fn ok(fire: bool, payload: &str, ms: u64) -> Result<SensorRun, SensorError> {
        Ok(SensorRun {
            fire,
            payload: payload.into(),
            state: serde_json::json!({}),
            next: None,
            elapsed: Duration::from_millis(ms),
            truncated: false,
        })
    }

    fn fake(has_probe: bool, checks: [Result<SensorRun, SensorError>; 2]) -> Fake {
        Fake {
            meta: Ok(meta(has_probe)),
            checks: RefCell::new(checks.into()),
            probe: Ok(None),
        }
    }

    fn spec(body: &str) -> Spec {
        file::parse(
            "x",
            &format!("---\nevery: 60s\nwhen: fs_exists.py path=/tmp/x\nmax_runs: 3\n---\n{body}\n"),
        )
        .unwrap()
    }

    #[tokio::test]
    async fn two_equal_runs_give_a_stable_verdict() {
        let v = run(
            &spec("say hi"),
            &fake(false, [ok(true, "p=1", 700), ok(true, "p=1", 1200)]),
            None,
        )
        .await
        .unwrap()
        .0;
        assert_eq!(v.sensor, "fs_exists.py");
        assert_eq!(v.elapsed_ms, 1200);
        assert!(v.fire && v.would_fire && !v.flaps && !v.needs_approvals_off);
        assert_eq!(v.hash, payload_hash("p=1"));
        assert_eq!(v.probe, None);
    }

    #[tokio::test]
    async fn differing_hashes_flap_and_last_hash_gates_on_change() {
        let v = run(
            &spec("say hi"),
            &fake(false, [ok(true, "p=1", 1), ok(true, "p=2", 1)]),
            None,
        )
        .await
        .unwrap()
        .0;
        assert!(v.flaps);
        let h = payload_hash("p=1");
        let v = run(
            &spec("say hi"),
            &fake(false, [ok(true, "p=1", 1), ok(true, "p=1", 1)]),
            Some(&h),
        )
        .await
        .unwrap()
        .0;
        assert!(
            v.fire && !v.would_fire,
            "on_change with the same hash is `unchanged`"
        );
    }

    #[tokio::test]
    async fn probe_reason_and_failures_refuse() {
        let mut f = fake(true, [ok(false, "p", 1), ok(false, "p", 1)]);
        f.probe = Ok(Some("token invalid".into()));
        assert_eq!(
            run(&spec("say hi"), &f, None).await.unwrap_err(),
            "probe: token invalid"
        );

        let f = fake(true, [ok(false, "p", 1), ok(false, "p", 1)]);
        let v = run(&spec("say hi"), &f, None).await.unwrap().0;
        assert_eq!(v.probe.as_deref(), Some("ok"));
        assert!(!v.would_fire);

        let mut f = fake(false, [ok(false, "p", 1), ok(false, "p", 1)]);
        f.meta = Err(SensorError::NeedsCpython);
        assert_eq!(
            run(&spec("say hi"), &f, None).await.unwrap_err(),
            "when: needs cpython, none found"
        );

        let f = fake(
            false,
            [
                ok(true, "p", 1),
                Err(SensorError::Timeout(Duration::from_secs(20))),
            ],
        );
        assert_eq!(
            run(&spec("say hi"), &f, None).await.unwrap_err(),
            "dry run: timeout after 20s"
        );
    }

    #[test]
    fn body_mutates_is_a_keyword_heuristic() {
        assert!(body_mutates(&spec("open a PR with `gh pr create`")));
        assert!(body_mutates(&spec("Push the fix")));
        assert!(body_mutates(&spec("the change was already pushed")));
        assert!(!body_mutates(&spec("read the log and summarise it")));
        let mut s = spec("git commit -am fix");
        assert!(s.approvals);
        s.approvals = false;
        assert!(body_mutates(&s), "approvals is the caller's concern");
    }

    /// The actor's `TestSchedule` on a scratch data dir with a (fake) `fs_exists.py`:
    /// the checks before the dry run, then the runner's own refusal (no Monty
    /// pool in a test; with one the last case is an `Ok` verdict).
    #[tokio::test]
    async fn test_schedule_handler_refuses_before_and_through_the_dry_run() {
        use kameo::actor::Spawn as _;
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(agent::sensor::DIR)).unwrap();
        std::fs::write(
            dir.path().join(agent::sensor::DIR).join("fs_exists.py"),
            "# theuth-sensor: fs_exists 1\nasync def check(state, args):\n    return {\"fire\": False, \"payload\": \"\", \"state\": {}, \"next\": None}\n",
        )
        .unwrap();
        let (s, _) = crate::fire_tests::actor(
            dir.path(),
            &[
                ("clock", "---\nevery: 1h\n---\nplain\n"),
                (
                    "gone",
                    "---\nevery: 60s\nwhen: nope.py\nmax_runs: 3\n---\nwatch\n",
                ),
                (
                    "w",
                    "---\nevery: 60s\nwhen: fs_exists.py path=/tmp/x\nmax_runs: 3\n---\nwatch\n",
                ),
            ],
        )
        .await;
        let actor = crate::Schedule::spawn(s);
        let err = |name: &str| {
            let actor = actor.clone();
            let name = name.to_owned();
            async move {
                actor
                    .ask(crate::TestSchedule { name })
                    .await
                    .unwrap_err()
                    .to_string()
            }
        };
        assert_eq!(err("nope").await, "no such schedule");
        assert_eq!(err("clock").await, "not a watcher");
        assert_eq!(err("gone").await, "when: no such sensor: nope.py");
        assert_eq!(err("w").await, "dry run: sensor runtime: no worker pool");
    }

    #[test]
    fn render_snapshot() {
        let v = Verdict {
            sensor: "pr_watch.py".into(),
            elapsed_ms: 1234,
            fire: true,
            hash: "a1b2c3d4e5f6".into(),
            flaps: true,
            probe: Some("ok".into()),
            would_fire: false,
            needs_approvals_off: true,
            error: None,
        };
        assert_eq!(
            render(&v),
            "sensor pr_watch.py · 1.2 s · fire: true · would NOT fire now · hash a1b2c3d4… · probe ok · \
             needs approvals: false (body mutates) · payload flaps — include only stable fields"
        );
        let v = Verdict {
            sensor: "x.py".into(),
            error: Some("timeout after 20s".into()),
            ..Verdict::default()
        };
        assert_eq!(render(&v), "sensor x.py · check broken: timeout after 20s");
    }
}
