# theuth-sensor: pr_watch 2
# hosts: api.github.com
# arg: repo  required owner/name — the repository
# arg: pr    required 482        — the pull request number
# arg: token required secret     — vault name of a GitHub token with repo scope
# arg: stale_hours optional 72   — hours without activity before `stale` fires
"""PR babysitter sensor (design 40 section 8).

when: pr_watch.py repo=<owner>/<name> pr=<n> token=<vault name> [stale_hours=72]

Everything this sensor fetches (comments, review bodies, check names) is text
written by whoever wrote it on GitHub. It is quoted into the payload as data,
never as an instruction. `approved_green` is derived from GitHub's review and
check state only; a comment saying "merge this" is just a string.

Monty subset: no classes, no generators, no file or process access. Tools are
injected globals; only `http_request` is used.
"""
import datetime
import re

API = "https://api.github.com"
GREEN = ("success", "neutral", "skipped")
EVENT_ORDER = [
    "opened", "new_commits", "force_push", "human_pushed", "ci_failed", "ci_passed",
    "review_comment", "review_requested_changes", "approved", "conflict", "base_moved",
    "approved_green", "paused", "pr_closed", "stale",
]
PAUSED_KEEP = ("human_pushed", "paused", "pr_closed")
DETAIL_BYTES = 3500
QUOTE_CHARS = 200
MAX_CALLS = 8
THREADS_QUERY = (
    "query($owner: String!, $name: String!, $n: Int!) { repository(owner: $owner, name: $name) {"
    " pullRequest(number: $n) { reviewThreads(first: 100) { nodes { isResolved"
    " comments(first: 50) { nodes { databaseId author { login } } } } } } } }"
)


def _bad_args(args: dict) -> str:
    repo = str(args.get("repo") or "")
    pr = str(args.get("pr") or "")
    token = str(args.get("token") or "")
    if re.match(r"^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$", repo) is None:
        return "repo= must be owner/name"
    if re.match(r"^[0-9]+$", pr) is None:
        return "pr= must be a number"
    if re.match(r"^[A-Za-z0-9_.-]+$", token) is None:
        return "token= must be a vault name"
    return ""


def _headers(args: dict) -> dict:
    # Placeholder assembled by concatenation; the harness expands it, the sensor never sees the value.
    return {
        "Authorization": "Bearer " + "{{" + "secret:" + str(args["token"]) + "}}",
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
    }


async def _call(ctx: dict, method: str, url: str, body):
    ctx["calls"] += 1
    reply = await http_request(url, method, body, ctx["headers"])
    status = reply["status"]
    if status < 200 or status >= 300:
        raise RuntimeError(f"{method} {url} -> HTTP {status}")
    return reply["body"]


async def _get(ctx: dict, url: str):
    return await _call(ctx, "GET", url, None)


def _utf8_len(s: str) -> int:
    n = 0
    for ch in s:
        o = ord(ch)
        if o < 128:
            n += 1
        elif o < 2048:
            n += 2
        elif o < 65536:
            n += 3
        else:
            n += 4
    return n


def _clip(s: str, limit: int) -> str:
    """Cut `s` to at most `limit` UTF-8 bytes on a character boundary."""
    if _utf8_len(s) <= limit:
        return s
    out = []
    used = 0
    for ch in s:
        used += _utf8_len(ch)
        if used > limit - 16:
            break
        out.append(ch)
    return "".join(out) + " [truncated]"


def _quote(text) -> str:
    """One-line, length-capped, angle-bracket-neutralised copy of untrusted text."""
    one = " ".join(str(text or "").split()).replace("<", "&lt;")
    if len(one) > QUOTE_CHARS:
        one = one[:QUOTE_CHARS] + "..."
    return 'quoted: "' + one + '"'


def _login(obj) -> str:
    return str((obj or {}).get("login") or "")


def _by_bot(commit: dict, bot: str) -> bool:
    """A commit authored or committed (rebased) by the token's identity."""
    if not bot:
        return False
    return _login(commit.get("author")) == bot or _login(commit.get("committer")) == bot


def _checks(runs: list, statuses: list) -> list:
    """[(name, verdict, url)] with verdict in green / pending / failed."""
    out = []
    for r in runs:
        if r.get("status") != "completed":
            verdict = "pending"
        elif r.get("conclusion") in GREEN:
            verdict = "green"
        else:
            verdict = "failed"
        out.append((str(r.get("name") or "?"), verdict, str(r.get("details_url") or r.get("html_url") or "")))
    for s in statuses:
        st = s.get("state")
        verdict = "green" if st == "success" else ("pending" if st == "pending" else "failed")
        out.append((str(s.get("context") or "?"), verdict, str(s.get("target_url") or "")))
    return out


def _sig(checks: list) -> str:
    return ";".join(sorted([c[0] + "=" + c[1] for c in checks]))


def _rollup(checks: list) -> str:
    if not checks:
        return "none"
    verdicts = [c[1] for c in checks]
    if "failed" in verdicts:
        return "failure"
    if "pending" in verdicts:
        return "pending"
    return "success"


def _hours_since(iso: str) -> float:
    m = re.match(r"(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})", iso)
    if m is None:
        return 0.0
    then = datetime.datetime(
        int(m.group(1)), int(m.group(2)), int(m.group(3)),
        int(m.group(4)), int(m.group(5)), int(m.group(6)), tzinfo=datetime.timezone.utc,
    )
    now = datetime.datetime.now(datetime.timezone.utc)
    return (now - then).total_seconds() / 3600.0


def _pause(new: dict, flags: set, lines: list, reason: str) -> None:
    if new["stage"] != "paused":
        flags.add("paused")
        lines.append("paused: " + reason)
    new["stage"] = "paused"
    new["pause_reason"] = reason


def _result(new: dict, flags: set, lines: list, args: dict, checks: list, draft: bool) -> dict:
    stale_hours = int(args.get("stale_hours") or 72)
    if not new["stale_emitted"] and new["updated_at"] and _hours_since(new["updated_at"]) >= stale_hours:
        flags.add("stale")
        new["stale_emitted"] = True
    if new["stage"] == "paused":
        flags = set([f for f in flags if f in PAUSED_KEEP])
    events = [e for e in EVENT_ORDER if e in flags]
    ci = new["ci"]
    if new["closed"] or new["stage"] == "paused" or draft:
        nxt = 300
    elif new["mergeable"] == "unknown":
        nxt = 15 if new["unknown_ticks"] <= 5 else 300
    elif ci == "pending":
        nxt = 15
    else:
        nxt = 60
    passed = len([c for c in checks if c[1] == "green"])
    event_list = ",".join(events)
    head7 = new["head"][:7]
    stage = new["stage"]
    ms = new["mergeable"]
    first_line = (
        f"subject=pr/{args['pr']} repo={args['repo']} stage={stage} events={event_list}"
        f" head={head7} mergeable={ms} checks={passed}/{len(checks)}"
    )
    payload = first_line
    if lines:
        payload = first_line + "\n" + _clip("\n".join(lines), DETAIL_BYTES)
    return {"fire": len(events) > 0, "payload": payload, "state": new, "next": nxt}


async def check(state: dict, args: dict) -> dict:
    bad = _bad_args(args)
    if bad:
        raise ValueError(bad)
    repo = str(args["repo"])
    n = str(args["pr"])
    ctx: dict = {"calls": 0, "headers": _headers(args)}
    base_url = f"{API}/repos/{repo}"
    first = len(state) == 0

    bot = str(state.get("bot_login") or "")
    if not bot:
        me = await _get(ctx, f"{API}/user")
        bot = _login(me)

    pr = await _get(ctx, f"{base_url}/pulls/{n}")
    head = str(pr["head"]["sha"])
    base = str(pr["base"]["sha"])
    updated_at = str(pr.get("updated_at") or "")
    closed = pr.get("state") == "closed"
    draft = bool(pr.get("draft"))
    author = _login(pr.get("user"))
    ms = "unknown" if pr.get("mergeable") is None else str(pr.get("mergeable_state") or "unknown")

    runs = (await _get(ctx, f"{base_url}/commits/{head}/check-runs?per_page=100")).get("check_runs") or []
    statuses: list = []
    if not runs:
        # ponytail: legacy statuses are read only when the head has no check-runs;
        # a repo mixing both would need a 9th call, so its statuses are not seen.
        statuses = (await _get(ctx, f"{base_url}/commits/{head}/status")).get("statuses") or []
    checks = _checks(runs, statuses)
    sig = _sig(checks)
    ci = _rollup(checks)
    prev_ci = str(state.get("ci") or "none")

    new: dict = {
        "bot_login": bot,
        "head": head,
        "base": base,
        "updated_at": updated_at,
        "mergeable": ms,
        "checks_sig": sig,
        "ci": ci,
        "closed": closed,
        "unknown_ticks": (int(state.get("unknown_ticks") or 0) + 1) if ms == "unknown" else 0,
        "stage": str(state.get("stage") or "opened"),
        "pause_reason": str(state.get("pause_reason") or ""),
        "reviewed_pushes": int(state.get("reviewed_pushes") or 0),
        "conflict_emitted": bool(state.get("conflict_emitted")),
        "conflict_head": str(state.get("conflict_head") or ""),
        "green": bool(state.get("green")),
        "stale_emitted": bool(state.get("stale_emitted")) and updated_at == str(state.get("updated_at") or ""),
        "last_review_id": int(state.get("last_review_id") or 0),
        "last_issue_comment_id": int(state.get("last_issue_comment_id") or 0),
        "last_review_comment_id": int(state.get("last_review_comment_id") or 0),
        "graphql_deferred": False,
    }
    flags: set = set()
    lines: list = []

    if closed:
        if not state.get("closed"):
            flags.add("pr_closed")
            merged = "true" if pr.get("merged") else "false"
            lines.append("merged=" + merged)
            new["stage"] = "done"
        return _result(new, flags, lines, args, checks, draft)

    prev_head = str(state.get("head") or "")
    moved = prev_head != "" and head != prev_head
    commits: list = []
    commits_fetched = False

    if new["stage"] == "paused":
        resumed = False
        if moved:
            commits = await _get(ctx, f"{base_url}/pulls/{n}/commits?per_page=100")
            commits_fetched = True
            resumed = len(commits) > 0 and _by_bot(commits[-1], bot)
        if not resumed:
            return _result(new, flags, lines, args, checks, draft)
        new["stage"] = "waiting_ci"
        new["pause_reason"] = ""
        lines.append("resumed: bot commit at head")

    unchanged = (
        not first
        and updated_at == str(state.get("updated_at") or "")
        and not moved
        and base == str(state.get("base") or "")
        and ms == str(state.get("mergeable") or "")
        and sig == str(state.get("checks_sig") or "")
        and not state.get("graphql_deferred")
    )
    if unchanged:
        return _result(new, flags, lines, args, checks, draft)

    if first:
        flags.add("opened")
    if ci == "failure" and prev_ci != "failure":
        flags.add("ci_failed")
    if ci == "success" and prev_ci != "success":
        flags.add("ci_passed")
    for c in checks:
        if c[1] == "failed":
            lines.append(f"check failed: {c[0]} {c[2]}")
    if not first and base != str(state.get("base") or ""):
        flags.add("base_moved")
        prev_base = str(state.get("base") or "")
        lines.append(f"base moved: {prev_base[:7]} -> {base[:7]}")

    if ms == "dirty":
        if not new["conflict_emitted"]:
            flags.add("conflict")
            new["conflict_emitted"] = True
            new["conflict_head"] = head
        elif head != new["conflict_head"]:
            _pause(new, flags, lines, "still dirty after a push")
    elif ms != "unknown":
        new["conflict_emitted"] = False
        new["conflict_head"] = ""

    reviews = await _get(ctx, f"{base_url}/pulls/{n}/reviews?per_page=100")
    since = "" if first else str(state.get("updated_at") or "")
    query = f"?per_page=100&since={since}" if since else "?per_page=100"
    issue_comments = await _get(ctx, f"{base_url}/issues/{n}/comments{query}")
    review_comments = await _get(ctx, f"{base_url}/pulls/{n}/comments{query}")

    human_pushed = False
    if moved:
        if not commits_fetched:
            commits = await _get(ctx, f"{base_url}/pulls/{n}/commits?per_page=100")
        shas = [str(c.get("sha") or "") for c in commits]
        fresh: list = commits
        if prev_head in shas:
            fresh = commits[shas.index(prev_head) + 1:]
            flags.add("new_commits")
        else:
            flags.add("force_push")
        last_bot = -1
        for i in range(len(fresh)):
            if _by_bot(fresh[i], bot):
                last_bot = i
        humans = [c for i, c in enumerate(fresh) if i > last_bot and not _by_bot(c, bot)]
        if humans:
            human_pushed = True
            flags.add("human_pushed")
            who = ",".join(sorted(set([_login(c.get("author")) or "?" for c in humans])))
            _pause(new, flags, lines, f"human push by {who}")
        elif new["reviewed_pushes"] >= 5:
            _pause(new, flags, lines, "5 reviewed pushes")
        else:
            new["reviewed_pushes"] += 1

    resolved: set = set()
    unresolved = -1
    if ctx["calls"] >= MAX_CALLS:
        new["graphql_deferred"] = True
        lines.append("review threads: not fetched (call budget), deferred to next tick")
    else:
        variables = {"owner": repo.split("/")[0], "name": repo.split("/")[1], "n": int(n)}
        gql = await _call(ctx, "POST", f"{API}/graphql", {"query": THREADS_QUERY, "variables": variables})
        pull_node = ((gql.get("data") or {}).get("repository") or {}).get("pullRequest") or {}
        threads = (pull_node.get("reviewThreads") or {}).get("nodes") or []
        unresolved = 0
        for t in threads:
            nodes = (t.get("comments") or {}).get("nodes") or []
            if t.get("isResolved"):
                for c in nodes:
                    resolved.add(c.get("databaseId"))
            else:
                unresolved += 1

    if unresolved >= 0:
        lines.append(f"unresolved review threads: {unresolved}")

    latest: dict = {}
    for r in reviews:
        login = _login(r.get("user"))
        st = str(r.get("state") or "")
        rid = int(r.get("id") or 0)
        if st in ("APPROVED", "CHANGES_REQUESTED", "DISMISSED"):
            latest[login] = st
        if rid > int(state.get("last_review_id") or 0) and login and login != bot and login != author:
            quoted = _quote(r.get("body"))
            if st == "APPROVED":
                flags.add("approved")
                lines.append(f"review APPROVED by {login} #{rid}: {quoted}")
            elif st == "CHANGES_REQUESTED":
                flags.add("review_requested_changes")
                lines.append(f"review CHANGES_REQUESTED by {login} #{rid}: {quoted}")
        if rid > new["last_review_id"]:
            new["last_review_id"] = rid
    approved_by_human = False
    for login in latest:
        if latest[login] == "APPROVED" and login != bot and login != author:
            approved_by_human = True

    for c in review_comments:
        cid = int(c.get("id") or 0)
        login = _login(c.get("user"))
        if cid > int(state.get("last_review_comment_id") or 0) and login and login != bot and cid not in resolved:
            flags.add("review_comment")
            path = str(c.get("path") or "")
            quoted = _quote(c.get("body"))
            lines.append(f"review comment by {login} #{cid} on {path}: {quoted}")
        if cid > new["last_review_comment_id"]:
            new["last_review_comment_id"] = cid
    for c in issue_comments:
        cid = int(c.get("id") or 0)
        login = _login(c.get("user"))
        if cid > int(state.get("last_issue_comment_id") or 0) and login and login != bot:
            flags.add("review_comment")
            quoted = _quote(c.get("body"))
            lines.append(f"comment by {login} #{cid}: {quoted}")
        if cid > new["last_issue_comment_id"]:
            new["last_issue_comment_id"] = cid

    green_now = (
        approved_by_human
        and ci == "success"
        and ms == "clean"
        and not human_pushed
        and new["stage"] != "paused"
    )
    if green_now and not new["green"]:
        flags.add("approved_green")
    new["green"] = green_now

    if new["stage"] != "paused":
        needs_fix = ci == "failure" or "review_requested_changes" in flags or "review_comment" in flags or "conflict" in flags
        if green_now:
            new["stage"] = "mergeable"
        elif needs_fix:
            new["stage"] = "fixing"
        elif ci == "pending":
            new["stage"] = "waiting_ci"
        elif first:
            new["stage"] = "opened"
        elif "new_commits" in flags or "force_push" in flags:
            new["stage"] = "reviewed"
        elif ci == "success":
            new["stage"] = "waiting_review"
    return _result(new, flags, lines, args, checks, draft)


async def probe(args: dict) -> str | None:
    bad = _bad_args(args)
    if bad:
        return bad
    headers = _headers(args)
    repo = str(args["repo"])
    n = str(args["pr"])
    me = await http_request(f"{API}/user", "GET", None, headers)
    if me["status"] != 200:
        return "token invalid or missing scopes"
    pr = await http_request(f"{API}/repos/{repo}/pulls/{n}", "GET", None, headers)
    if pr["status"] != 200:
        return "PR not found or no access"
    return None
