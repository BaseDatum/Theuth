"""pr_watch.py under CPython via monty_shim. Run: python3 -m unittest discover -s assets/sensors/tests"""
import datetime
import json
import re
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from monty_shim import SENSORS, FakeHttp, load, run  # noqa: E402

REPO = "owner/name"
N = "482"
ARGS = {"repo": REPO, "pr": N, "token": "gh_token"}
BOT = "theuth-bot"
AUTHOR = "alice"
REVIEWER = "bob"
H = [c * 40 for c in "abcdefgh"]
BASE = ["1" * 40, "2" * 40]


def iso_hours_ago(h):
    """`updated_at` relative to the wall clock: pr_watch._hours_since compares against now, so a
    pinned date goes `stale` (72 h) on its own; fresh fixtures must move with the clock."""
    t = datetime.datetime.now(datetime.timezone.utc) - datetime.timedelta(hours=h)
    return t.strftime("%Y-%m-%dT%H:%M:%SZ")


T0 = iso_hours_ago(3)
T1 = iso_hours_ago(2)
T2 = iso_hours_ago(1)
P = f"/repos/{REPO}"


def pull(head=H[0], base=BASE[0], state="open", mergeable=True, ms="clean", updated=T0, draft=False, merged=False):
    return {
        "state": state, "draft": draft, "merged": merged, "updated_at": updated,
        "mergeable": mergeable, "mergeable_state": ms,
        "head": {"sha": head}, "base": {"sha": base}, "user": {"login": AUTHOR},
    }


def check_run(name="ci", conclusion="success", status="completed"):
    return {"name": name, "status": status, "conclusion": conclusion, "details_url": f"https://ci.example/{name}"}


def commit(sha, author=AUTHOR, committer=None):
    return {"sha": sha, "author": {"login": author}, "committer": {"login": committer or author}}


def review(rid, login, state, body=""):
    return {"id": rid, "user": {"login": login}, "state": state, "body": body}


def comment(cid, login, body="", path=None):
    c = {"id": cid, "user": {"login": login}, "body": body}
    if path:
        c["path"] = path
    return c


def routes(pr=None, runs=None, statuses=(), reviews=(), issue_comments=(), review_comments=(), commits=(), threads=()):
    pr = pr or pull()
    runs = [check_run()] if runs is None else runs
    head = pr["head"]["sha"]
    nodes = [
        {"isResolved": resolved, "comments": {"nodes": [{"databaseId": i, "author": {"login": REVIEWER}} for i in ids]}}
        for resolved, ids in threads
    ]
    return {
        ("GET", "/user"): {"login": BOT},
        ("GET", f"{P}/pulls/{N}"): pr,
        ("GET", f"{P}/commits/{head}/check-runs"): {"total_count": len(runs), "check_runs": runs},
        ("GET", f"{P}/commits/{head}/status"): {"statuses": list(statuses)},
        ("GET", f"{P}/pulls/{N}/reviews"): list(reviews),
        ("GET", f"{P}/issues/{N}/comments"): list(issue_comments),
        ("GET", f"{P}/pulls/{N}/comments"): list(review_comments),
        ("GET", f"{P}/pulls/{N}/commits"): list(commits),
        ("POST", "/graphql"): {"data": {"repository": {"pullRequest": {"reviewThreads": {"nodes": nodes}}}}},
    }


def envelope(payload):
    first = payload.split("\n", 1)[0]
    return dict(tok.split("=", 1) for tok in first.split(" "))


def tick(state, **kw):
    """One check; asserts the global budgets on every call."""
    http = FakeHttp(routes(**kw))
    res = run(load("pr_watch.py", http)["check"](state, ARGS))
    assert len(http.calls) <= 8, http.calls
    assert len(res["payload"].encode("utf-8")) <= 4096
    assert len(json.dumps(res["state"]).encode("utf-8")) <= 16384
    for _, _, _, headers in http.calls:
        assert headers["Authorization"] == "Bearer {{secret:gh_token}}"
    return res, http


def events(res):
    return envelope(res["payload"])["events"].split(",") if envelope(res["payload"])["events"] else []


def opened():
    return tick({})[0]["state"]


def pushed(state, new_head, author=BOT, prev_present=True, **kw):
    prev = state["head"]
    commits = ([commit(prev)] if prev_present else []) + [commit(new_head, author=author)]
    return tick(state, pr=pull(head=new_head, updated=T1), commits=commits, **kw)


class Lifecycle(unittest.TestCase):
    def test_opened_first_run_and_envelope(self):
        res, http = tick({})
        self.assertTrue(res["fire"])
        env = envelope(res["payload"])
        self.assertEqual(env["subject"], "pr/482")
        self.assertEqual(env["repo"], REPO)
        self.assertEqual(env["stage"], "opened")
        self.assertEqual(env["head"], "a" * 7)
        self.assertEqual(env["mergeable"], "clean")
        self.assertEqual(env["checks"], "1/1")
        self.assertIn("opened", events(res))
        self.assertEqual(res["state"]["bot_login"], BOT)
        self.assertEqual(http.calls[0][1], "https://api.github.com/user")
        self.assertEqual(res["next"], 60)

    def test_idle_short_circuit(self):
        state = opened()
        res, http = tick(state)
        self.assertFalse(res["fire"])
        self.assertEqual(events(res), [])
        self.assertEqual(len(http.calls), 2)
        self.assertEqual(res["next"], 60)

    def test_new_commits_incremental(self):
        state = opened()
        res, _ = pushed(state, H[1])
        self.assertEqual(events(res), ["new_commits"])
        self.assertEqual(res["state"]["head"], H[1])
        self.assertEqual(res["state"]["reviewed_pushes"], 1)
        self.assertEqual(res["state"]["stage"], "reviewed")
        # the next push is again incremental against the new head
        res2, _ = pushed(res["state"], H[2])
        self.assertEqual(events(res2), ["new_commits"])
        self.assertEqual(res2["state"]["reviewed_pushes"], 2)

    def test_force_push_by_bot_is_not_human(self):
        state = opened()
        res, _ = pushed(state, H[1], prev_present=False)
        self.assertEqual(events(res), ["force_push"])
        self.assertEqual(res["state"]["stage"], "reviewed")

    def test_base_moved(self):
        state = opened()
        res, _ = tick(state, pr=pull(base=BASE[1], updated=T1))
        self.assertIn("base_moved", events(res))
        self.assertIn(f"base moved: {BASE[0][:7]} -> {BASE[1][:7]}", res["payload"])

    def test_pr_closed_once(self):
        state = opened()
        res, http = tick(state, pr=pull(state="closed", merged=True, updated=T1))
        self.assertEqual(events(res), ["pr_closed"])
        self.assertIn("merged=true", res["payload"])
        self.assertEqual(res["state"]["stage"], "done")
        self.assertEqual(res["next"], 300)
        self.assertEqual(len(http.calls), 2)
        res2, _ = tick(res["state"], pr=pull(state="closed", merged=True, updated=T1))
        self.assertFalse(res2["fire"])

    def test_draft_next_300(self):
        res, _ = tick({}, pr=pull(draft=True))
        self.assertEqual(res["next"], 300)

    def test_stale_once(self):
        res, _ = tick({}, pr=pull(updated="2020-01-01T00:00:00Z"))
        self.assertIn("stale", events(res))
        res2, _ = tick(res["state"], pr=pull(updated="2020-01-01T00:00:00Z"))
        self.assertEqual(events(res2), [])
        res3, _ = tick(res["state"], pr=pull(updated="2020-01-01T00:00:00Z"))
        self.assertEqual(events(res3), [])


class CI(unittest.TestCase):
    def test_ci_failed_then_passed(self):
        state = opened()
        res, _ = tick(state, runs=[check_run(conclusion="failure"), check_run("lint")])
        self.assertEqual(events(res), ["ci_failed"])
        self.assertEqual(envelope(res["payload"])["checks"], "1/2")
        self.assertIn("check failed: ci https://ci.example/ci", res["payload"])
        self.assertEqual(res["state"]["stage"], "fixing")
        res2, _ = tick(res["state"], runs=[check_run(), check_run("lint")])
        self.assertEqual(events(res2), ["ci_passed"])
        self.assertEqual(res2["state"]["stage"], "waiting_review")

    def test_pending_next_15(self):
        res, _ = tick({}, runs=[check_run(status="in_progress", conclusion=None)])
        self.assertEqual(res["next"], 15)
        self.assertEqual(res["state"]["stage"], "waiting_ci")
        self.assertNotIn("ci_passed", events(res))

    def test_legacy_status_when_no_check_runs(self):
        res, http = tick({}, runs=[], statuses=[{"context": "jenkins", "state": "failure", "target_url": "u"}])
        self.assertIn("ci_failed", events(res))
        self.assertIn("check failed: jenkins u", res["payload"])
        res2, http2 = tick(res["state"], runs=[], statuses=[{"context": "jenkins", "state": "failure", "target_url": "u"}])
        self.assertFalse(res2["fire"])
        self.assertEqual(len(http2.calls), 3)


class Comments(unittest.TestCase):
    def test_human_review_comment_fires_bot_ignored(self):
        state = opened()
        res, _ = tick(state, pr=pull(updated=T1), review_comments=[
            comment(10, REVIEWER, "please <rename> this\nnow", path="src/a.py"),
            comment(11, BOT, "done"),
        ], threads=[(False, [10, 11])])
        self.assertEqual(events(res), ["review_comment"])
        self.assertIn('review comment by bob #10 on src/a.py: quoted: "please &lt;rename> this now"', res["payload"])
        self.assertNotIn("#11", res["payload"])
        self.assertIn("unresolved review threads: 1", res["payload"])
        self.assertEqual(res["state"]["stage"], "fixing")
        # same comments again: high-water mark, no new event
        res2, _ = tick(res["state"], pr=pull(updated=T2), review_comments=[comment(10, REVIEWER, "x")])
        self.assertEqual(events(res2), [])

    def test_bot_only_comment_no_event(self):
        state = opened()
        res, _ = tick(state, pr=pull(updated=T1), issue_comments=[comment(20, BOT, "reviewed")])
        self.assertFalse(res["fire"])

    def test_resolved_thread_comment_ignored(self):
        state = opened()
        res, _ = tick(state, pr=pull(updated=T1), review_comments=[comment(10, REVIEWER, "old")], threads=[(True, [10])])
        self.assertEqual(events(res), [])

    def test_human_issue_comment_fires(self):
        state = opened()
        res, _ = tick(state, pr=pull(updated=T1), issue_comments=[comment(20, REVIEWER, "LGTM")])
        self.assertEqual(events(res), ["review_comment"])
        self.assertNotIn("approved", events(res))

    def test_requested_changes(self):
        state = opened()
        res, _ = tick(state, pr=pull(updated=T1), reviews=[review(5, REVIEWER, "CHANGES_REQUESTED", "no")])
        self.assertEqual(events(res), ["review_requested_changes"])
        self.assertEqual(res["state"]["stage"], "fixing")


class ApprovedGreen(unittest.TestCase):
    def test_all_four(self):
        state = opened()
        res, _ = tick(state, pr=pull(updated=T1), reviews=[review(5, REVIEWER, "APPROVED")])
        self.assertEqual(events(res), ["approved", "approved_green"])
        self.assertEqual(res["state"]["stage"], "mergeable")
        # transition only: an idle tick does not repeat it
        res2, _ = tick(res["state"], pr=pull(updated=T1), reviews=[review(5, REVIEWER, "APPROVED")])
        self.assertFalse(res2["fire"])

    def test_missing_approval(self):
        state = opened()
        res, _ = tick(state, pr=pull(updated=T1), reviews=[review(5, REVIEWER, "COMMENTED", "LGTM")])
        self.assertNotIn("approved_green", events(res))
        res, _ = tick(state, pr=pull(updated=T1), reviews=[review(5, AUTHOR, "APPROVED")])
        self.assertNotIn("approved_green", events(res))
        res, _ = tick(state, pr=pull(updated=T1), reviews=[review(5, BOT, "APPROVED")])
        self.assertNotIn("approved_green", events(res))
        res, _ = tick(state, pr=pull(updated=T1), reviews=[review(5, REVIEWER, "APPROVED"), review(6, REVIEWER, "CHANGES_REQUESTED")])
        self.assertNotIn("approved_green", events(res))

    def test_missing_green_checks(self):
        state = opened()
        res, _ = tick(state, pr=pull(updated=T1), reviews=[review(5, REVIEWER, "APPROVED")],
                      runs=[check_run(), check_run("lint", status="queued", conclusion=None)])
        self.assertNotIn("approved_green", events(res))
        res, _ = tick(state, pr=pull(updated=T1), reviews=[review(5, REVIEWER, "APPROVED")], runs=[])
        self.assertNotIn("approved_green", events(res))

    def test_missing_clean(self):
        state = opened()
        res, _ = tick(state, pr=pull(updated=T1, ms="blocked"), reviews=[review(5, REVIEWER, "APPROVED")])
        self.assertIn("approved", events(res))
        self.assertNotIn("approved_green", events(res))
        res, _ = tick(state, pr=pull(updated=T1, mergeable=None), reviews=[review(5, REVIEWER, "APPROVED")])
        self.assertNotIn("approved_green", events(res))

    def test_missing_no_human_push(self):
        state = opened()
        res, _ = pushed(state, H[1], author=AUTHOR, reviews=[review(5, REVIEWER, "APPROVED")])
        self.assertNotIn("approved_green", events(res))
        self.assertEqual(res["state"]["stage"], "paused")


class Mergeable(unittest.TestCase):
    def test_unknown_next_15_then_300(self):
        res, _ = tick({}, pr=pull(mergeable=None, ms="unknown"))
        self.assertEqual(res["state"]["mergeable"], "unknown")
        self.assertEqual(envelope(res["payload"])["mergeable"], "unknown")
        self.assertEqual(res["next"], 15)
        state = res["state"]
        for _ in range(5):
            res, http = tick(state, pr=pull(mergeable=None, ms="unknown"))
            self.assertEqual(len(http.calls), 2)
            state = res["state"]
        self.assertEqual(state["unknown_ticks"], 6)
        self.assertEqual(res["next"], 300)
        res, _ = tick(state, pr=pull(mergeable=True, ms="clean"))
        self.assertEqual(res["state"]["unknown_ticks"], 0)
        self.assertEqual(res["next"], 60)

    def test_unknown_derives_no_conflict(self):
        res, _ = tick({}, pr=pull(mergeable=None, ms="dirty"))
        self.assertNotIn("conflict", events(res))

    def test_conflict_once_then_paused(self):
        state = opened()
        res, _ = tick(state, pr=pull(updated=T1, mergeable=False, ms="dirty"))
        self.assertEqual(events(res), ["conflict"])
        self.assertEqual(res["state"]["stage"], "fixing")
        res2, _ = tick(res["state"], pr=pull(updated=T1, mergeable=False, ms="dirty"))
        self.assertFalse(res2["fire"])
        # the bot rebased (force push, bot as committer) and it is still dirty
        res3, http = tick(res2["state"], pr=pull(head=H[1], updated=T2, mergeable=False, ms="dirty"),
                          commits=[commit(H[1], committer=BOT)])
        self.assertEqual(events(res3), ["paused"])
        self.assertEqual(res3["state"]["pause_reason"], "still dirty after a push")
        self.assertEqual(res3["next"], 300)

    def test_conflict_resets_when_clean(self):
        state = opened()
        res, _ = tick(state, pr=pull(updated=T1, mergeable=False, ms="dirty"))
        res, _ = tick(res["state"], pr=pull(updated=T2, mergeable=True, ms="clean"))
        self.assertFalse(res["state"]["conflict_emitted"])
        res, _ = tick(res["state"], pr=pull(head=H[1], updated=T2, mergeable=False, ms="dirty"), commits=[commit(H[1], committer=BOT)])
        self.assertIn("conflict", events(res))
        self.assertNotIn("paused", events(res))


class Guards(unittest.TestCase):
    def test_human_pushed_paused_sticky_then_resume(self):
        state = opened()
        res, _ = pushed(state, H[1], author=AUTHOR)
        self.assertEqual(events(res), ["human_pushed", "paused"])
        self.assertEqual(res["state"]["stage"], "paused")
        self.assertIn("paused: human push by alice", res["payload"])
        self.assertEqual(res["next"], 300)
        # sticky: a new human comment and a failing check produce nothing
        res2, http = tick(res["state"], pr=pull(head=H[1], updated=T2), runs=[check_run(conclusion="failure")],
                          issue_comments=[comment(30, REVIEWER, "hello")])
        self.assertFalse(res2["fire"])
        self.assertEqual(res2["state"]["stage"], "paused")
        self.assertEqual(len(http.calls), 2)
        # another human push while paused: still paused, one extra call for the commit list
        res3, http = tick(res2["state"], pr=pull(head=H[2], updated=T2), commits=[commit(H[1]), commit(H[2])])
        self.assertFalse(res3["fire"])
        self.assertEqual(len(http.calls), 3)
        # closed while paused still reaches the body
        res4, _ = tick(res3["state"], pr=pull(head=H[2], state="closed", updated=T2))
        self.assertEqual(events(res4), ["pr_closed"])
        # the bot pushed after the user said resume
        res5, _ = pushed(res3["state"], H[3], author=BOT)
        self.assertEqual(events(res5), ["new_commits"])
        self.assertNotEqual(res5["state"]["stage"], "paused")
        self.assertIn("resumed: bot commit at head", res5["payload"])

    def test_human_then_bot_in_one_push_is_not_human(self):
        state = opened()
        res, _ = tick(state, pr=pull(head=H[2], updated=T1), commits=[commit(H[0]), commit(H[1], author=AUTHOR), commit(H[2], author=BOT)])
        self.assertEqual(events(res), ["new_commits"])

    def test_five_reviewed_pushes_then_paused(self):
        state = opened()
        for i in range(1, 6):
            res, _ = pushed(state, H[i])
            self.assertEqual(events(res), ["new_commits"], i)
            state = res["state"]
        self.assertEqual(state["reviewed_pushes"], 5)
        res, _ = pushed(state, H[6])
        self.assertEqual(events(res), ["paused"])
        self.assertEqual(res["state"]["pause_reason"], "5 reviewed pushes")
        self.assertEqual(res["next"], 300)


class Budgets(unittest.TestCase):
    def test_worst_case_defers_graphql(self):
        state = opened()
        del state["bot_login"]  # forces GET /user on a tick that also needs the commit list
        res, http = tick(state, pr=pull(head=H[1], updated=T1), runs=[], commits=[commit(H[0]), commit(H[1], author=BOT)],
                         review_comments=[comment(10, REVIEWER, "x")])
        self.assertEqual(len(http.calls), 8)
        self.assertNotIn("POST", [c[0] for c in http.calls])
        self.assertTrue(res["state"]["graphql_deferred"])
        self.assertIn("deferred", res["payload"])
        self.assertIn("review_comment", events(res))
        res2, http2 = tick(res["state"], pr=pull(head=H[1], updated=T1), runs=[])
        self.assertIn("POST", [c[0] for c in http2.calls])
        self.assertFalse(res2["state"]["graphql_deferred"])
        res3, http3 = tick(res2["state"], pr=pull(head=H[1], updated=T1), runs=[])
        self.assertEqual(len(http3.calls), 3)

    def test_sizes_with_50_comments(self):
        state = opened()
        body = "lorem ipsum " * 60
        res, _ = tick(state, pr=pull(updated=T1),
                      issue_comments=[comment(100 + i, REVIEWER, body) for i in range(25)],
                      review_comments=[comment(200 + i, REVIEWER, body, path="f.py") for i in range(25)],
                      threads=[(False, [200 + i]) for i in range(25)])
        payload = res["payload"].encode("utf-8")
        self.assertLessEqual(len(payload), 4096)
        self.assertGreater(len(payload), 3000)
        self.assertIn("[truncated]", res["payload"])
        self.assertLessEqual(len(json.dumps(res["state"]).encode("utf-8")), 16384)
        self.assertEqual(res["state"]["last_issue_comment_id"], 124)
        self.assertEqual(res["state"]["last_review_comment_id"], 224)
        self.assertIn("unresolved review threads: 25", res["payload"])

    def test_payload_unicode_clip(self):
        state = opened()
        res, _ = tick(state, pr=pull(updated=T1), issue_comments=[comment(100 + i, REVIEWER, "\u2603" * 300) for i in range(30)])
        self.assertLessEqual(len(res["payload"].encode("utf-8")), 4096)
        res["payload"].encode("utf-8")  # a clean boundary: no lone surrogates


class Probe(unittest.TestCase):
    def probe(self, routes_):
        http = FakeHttp(routes_)
        return run(load("pr_watch.py", http)["probe"](ARGS))

    def test_ok(self):
        self.assertIsNone(self.probe(routes()))

    def test_bad_token(self):
        r = routes()
        r[("GET", "/user")] = (401, {"message": "Bad credentials"})
        self.assertEqual(self.probe(r), "token invalid or missing scopes")

    def test_pr_missing(self):
        r = routes()
        del r[("GET", f"{P}/pulls/{N}")]
        self.assertEqual(self.probe(r), "PR not found or no access")

    def test_bad_args(self):
        ns = load("pr_watch.py", FakeHttp({}))
        self.assertEqual(run(ns["probe"]({"repo": "x", "pr": N, "token": "t"})), "repo= must be owner/name")
        with self.assertRaises(ValueError):
            run(ns["check"]({}, {"repo": REPO, "pr": "abc", "token": "t"}))

    def test_http_error_raises_strike(self):
        r = routes()
        r[("GET", f"{P}/pulls/{N}")] = (500, "oops")
        with self.assertRaises(RuntimeError):
            run(load("pr_watch.py", FakeHttp(r))["check"]({}, ARGS))


class Lint(unittest.TestCase):
    def test_monty_subset(self):
        src = (SENSORS / "pr_watch.py").read_text(encoding="utf-8")
        for pat in (r"^\s*import os\b", r"\byield\b", r"^\s*class .*\(", r"\bopen\(", r"\bsubprocess\b", "%",
                    r"\bimport (pathlib|asyncio|sys|threading)\b", r"\{\{secret:\w"):
            self.assertIsNone(re.search(pat, src, re.M), pat)
        header = src.split("\n")[:16]
        self.assertIn("# theuth-sensor: pr_watch 1", header)
        self.assertIn("# hosts: api.github.com", header)
        self.assertFalse(any(h.startswith("# needs:") for h in header))


if __name__ == "__main__":
    unittest.main()
