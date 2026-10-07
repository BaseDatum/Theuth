//! Placement (67 §8, decision 3): where and in what a task runs. Policy first
//! — the obvious cases need no model turn — and theuth's desk only when the
//! policy leaves more than one engine. Pure: [`decide`] reads the task, what
//! the engines advertise and the config; [`place`] adds the desk turn through
//! a caller-supplied function so the tool and the tests share one path.
//!
//! Rules, in order: (a) `where` / `harness` already on the task win; (b) a
//! repo path in the brief or `needs` (`~/…`, `/…`) or a project cwd narrows to
//! the engines that have it; (c) `browser` / `screen` / `display` needs an
//! engine with a display, `gpu` / `cuda` one with a GPU; (d) a `read_only`
//! owner always gets `harness: native` (an external harness cannot enforce
//! the ceiling); (e) repo work runs in `omp` when the engine advertises it,
//! else native. One engine left → placed, with the rules that led there as
//! the reason. Several → `tasks.place_prefer` breaks the tie, else the desk.
//! A pin (79 §4.9, the owner's `runs_in`) comes before all of it: the pinned
//! engine is the only candidate — placed there with the harness (d)–(e) pick,
//! or `Missing` when it cannot start work now; never a fallback engine.

use mesh::EngineStats;

use crate::types::Task;

/// One engine as the policy sees it: the name `spawn_subagent` resolves and
/// what it advertises.
#[derive(Clone, Debug)]
pub struct Engine {
    /// Node name (`mesh.name` / hostname; this engine's hostname for itself).
    pub name: String,
    /// What it advertises.
    pub stats: EngineStats,
}

/// What the policy needs.
pub struct Inputs<'a> {
    /// The task (brief, `where`, `harness`).
    pub task: &'a Task,
    /// The caller's `needs` argument.
    pub needs: Option<&'a str>,
    /// The owner agent's `tools` ceiling is `read_only`.
    pub read_only: bool,
    /// Working directories of sessions in the task's project.
    pub project_cwds: &'a [String],
    /// Engines that can start work now (this one included).
    pub engines: &'a [Engine],
    /// `tasks.place_prefer`.
    pub prefer: &'a [String],
    /// The owner's `runs_in`, resolved to an engine (79 §4.9); `None` = unpinned.
    pub pin: Option<Pin>,
}

/// A placement constraint: the task runs here or nowhere.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Pin {
    /// Engine name (`Engine.name`, what `spawn_subagent` resolves).
    Engine(String),
}

/// The answer written onto the task.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placement {
    /// Engine name.
    pub where_: String,
    /// `native` | `omp` | `opencode` | `claude` | …
    pub harness: String,
    /// One line for the thread.
    pub reason: String,
}

/// What the policy alone concludes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Exactly one answer.
    Placed(Placement),
    /// Several engines fit; theuth's desk picks one.
    Ambiguous {
        /// Engine names, policy order.
        candidates: Vec<String>,
        /// Harness the policy fixed, when it did (`read_only`, or set on the task).
        harness: Option<String>,
        /// The rules applied so far.
        why: String,
    },
    /// No engine fits: what is missing, as a `engine:<…>` blocker text.
    Missing(String),
}

/// Words in the brief that ask for a display.
const DISPLAY_WORDS: &[&str] = &["browser", "screen", "display", "screenshot"];
/// Words that ask for a GPU.
const GPU_WORDS: &[&str] = &["gpu", "cuda"];
/// Words that mark repo work when no path is named.
const REPO_WORDS: &[&str] = &[
    "repo",
    "repository",
    "git",
    "codebase",
    "crate",
    "pull request",
];

/// The policy (rules in the module doc).
#[must_use]
pub fn decide(i: &Inputs) -> Verdict {
    let mut why: Vec<String> = Vec::new();
    let pinned = i.pin.as_ref().map(|Pin::Engine(n)| n.as_str());
    if pinned.is_none()
        && let (Some(w), Some(h)) = (&i.task.where_, &i.task.harness)
    {
        return Verdict::Placed(Placement {
            where_: w.clone(),
            harness: h.clone(),
            reason: "set on the task".into(),
        });
    }
    let mut text = format!("{}\n{}", i.task.title, i.task.brief);
    if let Some(n) = i.needs {
        text.push('\n');
        text.push_str(n);
    }
    let lower = text.to_lowercase();
    let has_word = |words: &[&str]| words.iter().any(|w| lower.contains(w));

    let mut candidates: Vec<&Engine> = i.engines.iter().collect();
    if let Some(n) = pinned {
        candidates.retain(|e| e.name == n);
        if candidates.is_empty() {
            return Verdict::Missing(n.to_owned());
        }
        why.push(format!("pinned to {n} (runs_in)"));
    } else if let Some(w) = &i.task.where_ {
        candidates.retain(|e| e.name == *w || e.stats.id.0.starts_with(w.as_str()));
        if candidates.is_empty() {
            return Verdict::Missing(w.clone());
        }
        why.push(format!("`where: {w}` set on the task"));
    }

    // (b) a repo the work names. A pin skips the engine-narrowing rules (b)–(c):
    // there is nothing to choose among; `repo_work` still picks the harness.
    let mut paths: Vec<String> = mentioned_paths(&text);
    paths.extend(i.project_cwds.iter().cloned());
    let mut repo_work = has_word(REPO_WORDS);
    if !paths.is_empty() {
        repo_work = true;
    }
    if !paths.is_empty() && pinned.is_none() {
        let with_repo: Vec<&Engine> = candidates
            .iter()
            .copied()
            .filter(|e| {
                paths
                    .iter()
                    .any(|p| e.stats.repos.iter().any(|r| same_repo(r, p)))
            })
            .collect();
        if with_repo.is_empty() {
            return Verdict::Missing(format!("repo:{}", paths[0]));
        }
        why.push(format!(
            "repo {} is on {}",
            paths[0],
            with_repo
                .iter()
                .map(|e| e.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
        candidates = with_repo;
    }

    // (c) a display, a GPU.
    if pinned.is_none() && has_word(DISPLAY_WORDS) {
        candidates.retain(|e| e.stats.display);
        if candidates.is_empty() {
            return Verdict::Missing("display".into());
        }
        why.push("needs a display".into());
    }
    if pinned.is_none() && has_word(GPU_WORDS) {
        candidates.retain(|e| e.stats.gpu);
        if candidates.is_empty() {
            return Verdict::Missing("gpu".into());
        }
        why.push("needs a gpu".into());
    }

    // (d) the ceiling, or a harness set on the task.
    let fixed: Option<String> = match (&i.task.harness, i.read_only) {
        (_, true) => {
            why.push("owner is read_only → native".into());
            Some("native".into())
        }
        (Some(h), false) => {
            if h != "native" {
                candidates.retain(|e| e.stats.harnesses.iter().any(|x| x == h));
                if candidates.is_empty() {
                    return Verdict::Missing(format!("harness:{h}"));
                }
            }
            why.push(format!("`harness: {h}` set on the task"));
            Some(h.clone())
        }
        (None, false) => None,
    };

    // (e) the harness for one engine.
    let harness_on = |e: &Engine| -> String {
        if let Some(h) = &fixed {
            return h.clone();
        }
        if repo_work && e.stats.harnesses.iter().any(|h| h == "omp") {
            return "omp".into();
        }
        "native".into()
    };

    let chosen = match candidates.len() {
        0 => return Verdict::Missing("none".into()),
        1 => candidates[0],
        _ => match i
            .prefer
            .iter()
            .find_map(|p| candidates.iter().find(|e| e.name == *p))
        {
            Some(e) => {
                why.push(format!("{} preferred (tasks.place_prefer)", e.name));
                e
            }
            None => {
                return Verdict::Ambiguous {
                    candidates: candidates.iter().map(|e| e.name.clone()).collect(),
                    harness: fixed,
                    why: why.join("; "),
                };
            }
        },
    };
    let harness = harness_on(chosen);
    if fixed.is_none() {
        why.push(if harness == "omp" {
            "repo work → omp".into()
        } else {
            "native".into()
        });
    }
    if why.len() == 1 && candidates.len() == 1 && i.engines.len() == 1 {
        why.insert(0, "the only engine".into());
    }
    Verdict::Placed(Placement {
        where_: chosen.name.clone(),
        harness,
        reason: why.join("; "),
    })
}

/// Decide; on ambiguity ask theuth's desk with [`desk_prompt`] and read its
/// JSON answer. `desk` is one turn of the desk: prompt in, reply text out.
/// `Err` is the blocker text (`engine:<…>`) or why the desk did not answer.
pub async fn place<F, Fut>(i: &Inputs<'_>, desk: F) -> Result<Placement, String>
where
    F: FnOnce(String) -> Fut,
    Fut: Future<Output = Result<String, String>>,
{
    match decide(i) {
        Verdict::Placed(p) => Ok(p),
        Verdict::Missing(m) => Err(format!("engine:{m}")),
        Verdict::Ambiguous {
            candidates,
            harness,
            why,
        } => {
            let reply = desk(desk_prompt(i, &candidates, harness.as_deref(), &why)).await?;
            parse_desk_reply(&reply, &candidates, harness.as_deref())
        }
    }
}

/// The one-turn question for theuth's desk.
#[must_use]
pub fn desk_prompt(i: &Inputs, candidates: &[String], harness: Option<&str>, why: &str) -> String {
    let mut s = format!(
        "Place task {} — {}. Several engines fit; pick one. Candidates (name: class, harnesses, repos, display, gpu):\n",
        i.task.id, i.task.title
    );
    for e in i.engines.iter().filter(|e| candidates.contains(&e.name)) {
        let st = &e.stats;
        s.push_str(&format!(
            "- {}: {:?}, harnesses {}, repos {}, display {}, gpu {}\n",
            e.name,
            st.class,
            st.harnesses.join(","),
            if st.repos.is_empty() {
                "-".to_owned()
            } else {
                st.repos.join(",")
            },
            st.display,
            st.gpu
        ));
    }
    if !why.is_empty() {
        s.push_str(&format!("Policy so far: {why}.\n"));
    }
    match harness {
        Some(h) => s.push_str(&format!("The harness is fixed: {h}.\n")),
        None => {
            s.push_str("Harness: native, or one the engine advertises (omp / opencode / claude).\n")
        }
    }
    if let Some(n) = i.needs {
        s.push_str(&format!("Needs: {n}\n"));
    }
    s.push_str(&format!("Brief:\n{}\n", i.task.brief));
    s.push_str(
        "Answer with JSON only, as a reply to this message: {\"where\": \"<engine name>\", \"harness\": \"<harness>\", \"reason\": \"<one line>\"}",
    );
    s
}

/// The desk's answer: the first `{…}` in `reply` as `{where, harness, reason}`;
/// `where` must be a candidate, `harness` defaults to `fixed` or `native`.
pub fn parse_desk_reply(
    reply: &str,
    candidates: &[String],
    fixed: Option<&str>,
) -> Result<Placement, String> {
    #[derive(serde::Deserialize)]
    struct Answer {
        #[serde(rename = "where")]
        where_: String,
        #[serde(default)]
        harness: Option<String>,
        #[serde(default)]
        reason: Option<String>,
    }
    let (start, end) = (reply.find('{'), reply.rfind('}'));
    let json = match (start, end) {
        (Some(a), Some(b)) if a < b => &reply[a..=b],
        _ => return Err(format!("theuth's desk answered without JSON: {reply}")),
    };
    let a: Answer = serde_json::from_str(json)
        .map_err(|e| format!("theuth's desk answered malformed JSON ({e}): {json}"))?;
    let where_ = a.where_.trim();
    let Some(where_) = candidates.iter().find(|c| c.eq_ignore_ascii_case(where_)) else {
        return Err(format!(
            "theuth's desk chose `{where_}`, not one of {}",
            candidates.join(", ")
        ));
    };
    let harness = match fixed {
        Some(h) => h.to_owned(),
        None => a
            .harness
            .filter(|h| !h.trim().is_empty())
            .unwrap_or_else(|| "native".into()),
    };
    Ok(Placement {
        where_: where_.clone(),
        harness,
        reason: format!(
            "theuth's desk: {}",
            a.reason.unwrap_or_else(|| "no reason given".into())
        ),
    })
}

/// Absolute and home-relative paths mentioned in `text`, trailing
/// punctuation and quoting stripped, in order of appearance, deduplicated.
fn mentioned_paths(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tok in text.split(|c: char| {
        c.is_whitespace() || matches!(c, '`' | '"' | '\'' | '(' | ')' | '<' | '>' | ',' | ';')
    }) {
        let t = tok.trim_end_matches(['.', ':', '!', '?']);
        let is_path =
            (t.starts_with("~/") || (t.starts_with('/') && t.len() > 1)) && !t.starts_with("//");
        if is_path && !out.iter().any(|p| p == t) {
            out.push(t.to_owned());
        }
    }
    out
}

/// `repo` (an absolute git root on some engine) is the repo `path` names:
/// equal, or one under the other; a `~/…` path matches by suffix (the home
/// directory differs per engine), itself or one of its parents.
fn same_repo(repo: &str, path: &str) -> bool {
    let repo = repo.trim_end_matches('/');
    let path = path.trim_end_matches('/');
    if let Some(rel) = path.strip_prefix('~') {
        return repo.ends_with(rel)
            || rel
                .match_indices('/')
                .skip(1)
                .any(|(i, _)| repo.ends_with(&rel[..i]));
    }
    repo == path
        || path
            .strip_prefix(repo)
            .is_some_and(|rest| rest.starts_with('/'))
        || repo
            .strip_prefix(path)
            .is_some_and(|rest| rest.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use mesh::{EngineClass, EngineId};

    use super::*;
    use crate::types::{Owner, Priority, TaskId, TaskState};

    fn engine(
        name: &str,
        class: EngineClass,
        harnesses: &[&str],
        repos: &[&str],
        display: bool,
        gpu: bool,
    ) -> Engine {
        Engine {
            name: name.into(),
            stats: EngineStats {
                id: EngineId(format!("{name}-id-0123456789")),
                uptime_since_ms: 0,
                class,
                availability_30d: 1.0,
                harnesses: harnesses.iter().map(|s| (*s).to_owned()).collect(),
                repos: repos.iter().map(|s| (*s).to_owned()).collect(),
                display,
                gpu,
                accelerator: None,
            },
        }
    }

    fn fleet() -> Vec<Engine> {
        vec![
            engine(
                "mac",
                EngineClass::Laptop,
                &["native", "omp"],
                &["/Users/sky/dev/theuth"],
                true,
                true,
            ),
            engine(
                "build-a",
                EngineClass::Server,
                &["native", "omp"],
                &["/srv/sre-argo", "/srv/theuth"],
                false,
                false,
            ),
            engine(
                "box",
                EngineClass::Container,
                &["native"],
                &[],
                false,
                false,
            ),
        ]
    }

    fn task(brief: &str) -> Task {
        Task {
            id: TaskId("0190a1b2-0000-7000-8000-000000000001".into()),
            title: "t".into(),
            brief: brief.into(),
            owner: Owner::Agent("ops".into()),
            state: TaskState::Todo,
            reviewer: None,
            blockers: vec![],
            blocks: vec![],
            parent: None,
            project: None,
            where_: None,
            harness: None,
            external: vec![],
            thread: None,
            checkout: None,
            schedule: None,
            created_by: "user".into(),
            created_ms: 0,
            updated_ms: 0,
            no_progress_count: 0,
            next_wake_after_ms: None,
            done_when: None,
            attachments: vec![],
            priority: Priority::Medium,
        }
    }

    fn inputs<'a>(task: &'a Task, engines: &'a [Engine], prefer: &'a [String]) -> Inputs<'a> {
        Inputs {
            task,
            needs: None,
            read_only: false,
            project_cwds: &[],
            engines,
            prefer,
            pin: None,
        }
    }

    fn pinned<'a>(task: &'a Task, engines: &'a [Engine], to: &str) -> Inputs<'a> {
        Inputs {
            pin: Some(Pin::Engine(to.into())),
            ..inputs(task, engines, &[])
        }
    }

    fn placed(v: Verdict) -> Placement {
        match v {
            Verdict::Placed(p) => p,
            other => panic!("expected Placed, got {other:?}"),
        }
    }

    #[test]
    fn pin_wins_over_every_better_engine() {
        let mut f = fleet();
        // A VM with the repo nowhere near it and no display; `build-a` has the repo, `mac` the display.
        f.push(engine(
            "vm-ops",
            EngineClass::Vm,
            &["native", "omp"],
            &[],
            false,
            false,
        ));
        let t = task("fix the flaky test in /srv/theuth and screenshot the browser");
        let p = placed(decide(&pinned(&t, &f, "vm-ops")));
        assert_eq!(p.where_, "vm-ops");
        assert_eq!(p.harness, "omp", "repo work → omp on the pinned engine");
        assert!(
            p.reason.starts_with("pinned to vm-ops (runs_in)"),
            "{}",
            p.reason
        );

        // `where`/`harness` set on the task do not beat the pin.
        let mut t2 = task("anything");
        t2.where_ = Some("build-a".into());
        t2.harness = Some("omp".into());
        let p = placed(decide(&pinned(&t2, &f, "vm-ops")));
        assert_eq!((p.where_.as_str(), p.harness.as_str()), ("vm-ops", "omp"));

        // read_only still forces native on the pinned engine.
        let i = Inputs {
            read_only: true,
            ..pinned(&t, &f, "vm-ops")
        };
        assert_eq!(placed(decide(&i)).harness, "native");
    }

    #[test]
    fn pinned_engine_absent_is_missing_never_a_fallback() {
        let f = fleet();
        let t = task("plain work");
        assert_eq!(
            decide(&pinned(&t, &f, "vm-ops")),
            Verdict::Missing("vm-ops".into())
        );
        // Even with `place_prefer` naming an available engine.
        let prefer = ["mac".to_owned()];
        let i = Inputs {
            prefer: &prefer,
            ..pinned(&t, &f, "vm-ops")
        };
        assert_eq!(decide(&i), Verdict::Missing("vm-ops".into()));
    }

    /// (a) both fields set → exactly those, nothing else consulted.
    #[test]
    fn explicit_where_and_harness_win() {
        let mut t = task("anything");
        t.where_ = Some("nowhere".into());
        t.harness = Some("claude".into());
        let p = placed(decide(&inputs(&t, &fleet(), &[])));
        assert_eq!(
            (p.where_.as_str(), p.harness.as_str()),
            ("nowhere", "claude")
        );
        assert_eq!(p.reason, "set on the task");
    }

    /// (a) `where` alone narrows to that engine; (e) picks its harness.
    #[test]
    fn explicit_where_alone_narrows_and_harness_follows() {
        let mut t = task("fix the flaky deploy in the repo");
        t.where_ = Some("build-a".into());
        let p = placed(decide(&inputs(&t, &fleet(), &[])));
        assert_eq!((p.where_.as_str(), p.harness.as_str()), ("build-a", "omp"));
        t.where_ = Some("nope".into());
        assert_eq!(
            decide(&inputs(&t, &fleet(), &[])),
            Verdict::Missing("nope".into())
        );
        // (a) `harness` alone keeps only engines that have it.
        let mut t = task("write the runbook");
        t.harness = Some("omp".into());
        assert!(
            matches!(&decide(&inputs(&t, &fleet(), &[])), Verdict::Ambiguous { candidates, harness: Some(h), .. } if candidates == &["mac", "build-a"] && h == "omp")
        );
        t.harness = Some("opencode".into());
        assert_eq!(
            decide(&inputs(&t, &fleet(), &[])),
            Verdict::Missing("harness:opencode".into())
        );
    }

    /// (b) a path on exactly one engine → that engine; (e) omp there.
    #[test]
    fn repo_on_one_engine_places_there_in_omp() {
        let t = task("Fix the flaky deploy in `/srv/sre-argo/apps/web`.");
        let p = placed(decide(&inputs(&t, &fleet(), &[])));
        assert_eq!((p.where_.as_str(), p.harness.as_str()), ("build-a", "omp"));
        assert!(
            p.reason
                .contains("repo /srv/sre-argo/apps/web is on build-a"),
            "{}",
            p.reason
        );
        assert!(p.reason.ends_with("repo work → omp"), "{}", p.reason);
        // A `~/` path matches by suffix; on omp-less `box` it would be native.
        let t = task("bump deps in ~/dev/theuth");
        let p = placed(decide(&inputs(&t, &fleet(), &[])));
        assert_eq!(p.where_, "mac");
        // A path nobody has is a blocker.
        let t = task("look at /opt/elsewhere");
        assert_eq!(
            decide(&inputs(&t, &fleet(), &[])),
            Verdict::Missing("repo:/opt/elsewhere".into())
        );
    }

    /// (b) a project whose sessions' cwd is known counts as a mentioned path;
    /// a repo on two engines is ambiguous until `place_prefer` speaks.
    #[test]
    fn project_cwd_and_prefer() {
        let t = task("continue the refactor");
        let f = fleet();
        let cwds = ["/srv/theuth".to_owned()];
        let i = Inputs {
            project_cwds: &cwds,
            ..inputs(&t, &f, &[])
        };
        let p = placed(decide(&i));
        assert_eq!((p.where_.as_str(), p.harness.as_str()), ("build-a", "omp"));
        let cwds = ["/Users/sky/dev/theuth".to_owned(), "/srv/theuth".to_owned()];
        let i = Inputs {
            project_cwds: &cwds,
            ..inputs(&t, &f, &[])
        };
        assert!(
            matches!(&decide(&i), Verdict::Ambiguous { candidates, .. } if candidates == &["mac", "build-a"])
        );
        let prefer = ["build-a".to_owned()];
        let i = Inputs {
            prefer: &prefer,
            ..i
        };
        let p = placed(decide(&i));
        assert_eq!(p.where_, "build-a");
        assert!(
            p.reason.contains("preferred (tasks.place_prefer)"),
            "{}",
            p.reason
        );
    }

    /// (c) display and gpu words filter; missing → blocker.
    #[test]
    fn display_and_gpu_needs() {
        let t = task("Open the browser and check the dashboard renders");
        let p = placed(decide(&inputs(&t, &fleet(), &[])));
        assert_eq!((p.where_.as_str(), p.harness.as_str()), ("mac", "native"));
        assert!(p.reason.contains("needs a display"), "{}", p.reason);
        let mut t = task("train on the GPU");
        assert_eq!(placed(decide(&inputs(&t, &fleet(), &[]))).where_, "mac");
        t.where_ = Some("box".into());
        assert_eq!(
            decide(&inputs(&t, &fleet(), &[])),
            Verdict::Missing("gpu".into())
        );
        let no_display = fleet()
            .into_iter()
            .filter(|e| !e.stats.display)
            .collect::<Vec<_>>();
        let t = task("take a screenshot of the login screen");
        assert_eq!(
            decide(&inputs(&t, &no_display, &[])),
            Verdict::Missing("display".into())
        );
    }

    /// (d) a read_only owner is native even for repo work in omp territory.
    #[test]
    fn read_only_owner_is_always_native() {
        let t = task("audit /srv/sre-argo for secrets in git history");
        let f = fleet();
        let i = Inputs {
            read_only: true,
            ..inputs(&t, &f, &[])
        };
        let p = placed(decide(&i));
        assert_eq!(
            (p.where_.as_str(), p.harness.as_str()),
            ("build-a", "native")
        );
        assert!(p.reason.contains("read_only → native"), "{}", p.reason);
    }

    /// (e) repo words without a path: omp where advertised, native elsewhere;
    /// a lone engine is placed without a desk turn.
    #[test]
    fn harness_preference_and_lone_engine() {
        let only = vec![engine(
            "box",
            EngineClass::Container,
            &["native"],
            &[],
            false,
            false,
        )];
        let t = task("clean up the repo's CI");
        let p = placed(decide(&inputs(&t, &only, &[])));
        assert_eq!((p.where_.as_str(), p.harness.as_str()), ("box", "native"));
        assert!(p.reason.starts_with("the only engine"), "{}", p.reason);
        let only = vec![engine(
            "solo",
            EngineClass::Server,
            &["native", "omp"],
            &[],
            false,
            false,
        )];
        assert_eq!(placed(decide(&inputs(&t, &only, &[]))).harness, "omp");
        let t = task("draft the quarterly summary");
        assert_eq!(placed(decide(&inputs(&t, &only, &[]))).harness, "native");
        assert_eq!(
            decide(&inputs(&t, &[], &[])),
            Verdict::Missing("none".into())
        );
    }

    /// The ambiguous case is one desk turn: the prompt names the candidates,
    /// the JSON reply is validated against them; a bad or absent answer is
    /// an error that names the candidates.
    #[tokio::test]
    async fn ambiguous_asks_a_mocked_desk() {
        let t = task("draft the incident write-up");
        let f = fleet();
        let i = inputs(&t, &f, &[]);
        let seen = std::sync::Mutex::new(String::new());
        let p = place(&i, |prompt| {
            *seen.lock().unwrap() = prompt;
            async {
                Ok(
                    r#"Sure — {"where": "Build-A", "harness": "omp", "reason": "always on"}"#
                        .to_owned(),
                )
            }
        })
        .await
        .unwrap();
        assert_eq!((p.where_.as_str(), p.harness.as_str()), ("build-a", "omp"));
        assert_eq!(p.reason, "theuth's desk: always on");
        let prompt = seen.lock().unwrap().clone();
        for name in ["mac", "build-a", "box"] {
            assert!(prompt.contains(&format!("- {name}:")), "{prompt}");
        }
        assert!(prompt.contains("Answer with JSON only"), "{prompt}");

        let e = place(&i, |_| async { Ok("I would say the mac".to_owned()) })
            .await
            .unwrap_err();
        assert!(e.contains("without JSON"), "{e}");
        let e = place(&i, |_| async { Ok(r#"{"where": "moon"}"#.to_owned()) })
            .await
            .unwrap_err();
        assert!(e.contains("not one of mac, build-a, box"), "{e}");
        let e = place(&i, |_| async { Err("no desk".to_owned()) })
            .await
            .unwrap_err();
        assert_eq!(e, "no desk");

        // A fixed harness (read_only) is kept whatever the desk says.
        let i = Inputs {
            read_only: true,
            ..inputs(&t, &f, &[])
        };
        let p = place(&i, |_| async {
            Ok(r#"{"where":"mac","harness":"omp"}"#.to_owned())
        })
        .await
        .unwrap();
        assert_eq!((p.where_.as_str(), p.harness.as_str()), ("mac", "native"));
        assert!(p.reason.ends_with("no reason given"), "{}", p.reason);

        // Policy answers need no desk at all.
        let t = task("fix /srv/sre-argo");
        let p = place(&inputs(&t, &f, &[]), |_| async {
            unreachable!("policy decided")
        })
        .await
        .unwrap();
        assert_eq!(p.where_, "build-a");
        let t = task("look at /nowhere");
        assert_eq!(
            place(&inputs(&t, &f, &[]), |_| async { unreachable!() })
                .await
                .unwrap_err(),
            "engine:repo:/nowhere"
        );
    }

    #[test]
    fn paths_and_repo_matching() {
        assert_eq!(
            mentioned_paths("see `/a/b`, (~/c/d) and /e/f. Not //url or / alone."),
            ["/a/b", "~/c/d", "/e/f"]
        );
        assert!(same_repo("/srv/x", "/srv/x"));
        assert!(same_repo("/srv/x", "/srv/x/sub/dir"));
        assert!(same_repo("/srv/x/sub", "/srv/x"));
        assert!(!same_repo("/srv/xy", "/srv/x"));
        assert!(same_repo("/Users/sky/dev/theuth", "~/dev/theuth"));
        assert!(!same_repo("/Users/sky/dev/theuth", "~/dev/other"));
    }
}
