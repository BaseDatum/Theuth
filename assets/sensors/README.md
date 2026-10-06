# Sensors (design doc 40)

A sensor is one Python file in `<data_dir>/sensors/` that a `when:` schedule
names: `when: pr_watch.py repo=owner/name pr=482 token=gh_token`. Built-ins
shipped from this directory are written there on first run; the user may add
their own. Non-recursive, `*.py` only, dotfiles ignored (`.shim.py` is the
CPython shim, not a sensor).

## Header (first 16 lines; unknown `# key:` lines are ignored)

```python
# theuth-sensor: pr_watch 1          # built-ins only: <name> <version>; drives the update rule (40 §4.5)
# needs: cpython                     # optional: run as a child of a PATH Python via .shim.py, not Monty (40 §1.3)
# hosts: api.github.com,github.com   # optional: hosts http_request may POST to; GET is open (40 §4.3)
```

## Contract (40 §1.4)

```python
async def check(state: dict, args: dict) -> dict:
    # state: what this sensor returned last time (<= 16 KiB JSON); {} on the first run;
    #        committed only after the fired run succeeds (40 §1.7)
    # args:  key=value tokens from the `when:` line; args["cwd"] is the file's cwd
    # returns {"fire": bool, "payload": str (<= 4 KiB), "state": dict, "next": int | None}
    #   payload's first line is the envelope: `subject=… stage=… events=a,b` key=value tokens (40 §1.5)
    #   next: seconds until the next check; the gate clamps it to [every, 10 × every]

async def probe(args: dict) -> str | None:   # optional (40 §1.12)
    # creation-time reachability: token valid, PR exists, cwd is a clone; None = nothing to say
```

## Rules

- Read-only: every mutation belongs in the model's turn. Monty sensors get
  `http_request` (GET anywhere; POST only to `# hosts:`), `read_file`, `glob`,
  `list_dir` (rooted at `cwd`) and `knowledge_search`; anything else fails the
  type check at load. Secret placeholders `{{secret:<name>}}` expand in
  `http_request` headers only.
- Budgets (40 §5): wall clock `timeout:` (default 20 s), 64 MiB heap, 8 tool
  calls, 1 MiB fetched, payload 4 KiB (truncated), state 16 KiB (rejected).
- Everything the sensor fetched is data, never instruction: the payload reaches
  the model inside `<when>` (40 §7). Say so in the docstring.
- `http_request` returns `{url, status, headers, body, truncated, took_ms}`
  and raises `RuntimeError` on a non-2xx reply or a body cut at 1 MiB; a
  disallowed method/host raises `PermissionError`. Positional order:
  `http_request(url, method, body, headers)`.

## CPython sensors (`# needs: cpython`, 40 §4.4)

Run as `python3 <sensors>/.shim.py <sensor.py>` in the file's cwd: own
process group / Job Object, `nice 10`, killed as a tree at `timeout:`. The
shim exec's the file with `http_request`, `read_file`, `glob`, `list_dir` on
urllib/os under the same rules (`THEUTH_SENSOR_HOSTS` carries `# hosts:`);
`knowledge_search` is not available. Request on stdin
(`{"fn", "state", "args"}`), args also as `THEUTH_SENSOR_<KEY>`, result JSON
on stdout's last line; a non-zero exit is `check broken: exit N: <tail>`.
**No secrets in v1**: a cpython sensor whose `when:` args mention a
`{{secret:…}}` placeholder fails with `cpython sensors cannot use secrets in
v1`. Such a sensor talks to the world with its own libraries; its header must
say `# hosts:` is not enforced.

## Built-ins and updates (40 §4.5)

`fs_exists.py path=<glob>` fires while the glob matches under cwd (the
headless check of 40 §9). `install_builtins` writes the shipped files on
start and records `sensors.json` (`name → {version, sha256}`); a built-in is
rewritten only when the shipped `# theuth-sensor:` version is newer *and* the
file's sha256 is the one recorded — an edited copy is never overwritten (log
`sensor: <file> (edited; shipped N available)`); delete or rename it to take
the update. `.shim.py` is harness-owned and rewritten whenever it differs.

## pr_watch

`when: pr_watch.py repo=<owner>/<name> pr=<n> token=<vault name> [stale_hours=72]`

Envelope: `subject=pr/<n> repo=<repo> stage=<stage> events=<a,b> head=<sha7>
mergeable=<state> checks=<pass>/<total>`; detail lines follow (failing checks,
new reviews / comments quoted as data, unresolved thread count, base move).

Per tick: `GET pulls/<n>`, `GET commits/<head>/check-runs` (`…/status` when the
head has no check-runs); unchanged `updated_at` + head + base + mergeable +
check signature → 2 calls, `fire: false`. Otherwise reviews, issue comments,
review comments (`since=` previous `updated_at`), the commit list when the head
moved, and one GraphQL `POST` for review threads — deferred to the next tick
(`state.graphql_deferred`) when it would be the 9th call. `GET /user` once,
cached as `state.bot_login`.

Events (in order): `opened new_commits force_push human_pushed ci_failed
ci_passed review_comment review_requested_changes approved conflict base_moved
approved_green paused pr_closed stale`. `paused` is emitted on the transition
into stage `paused` (human push, 6th push after 5 reviewed, still dirty after a
push); paused is sticky — only `pr_closed` gets through — until a commit by
`bot_login` is at the head. `approved_green` needs a human non-author approval,
every check green, `mergeable_state == clean` and no human commit since the
bot's last one; it fires on the transition only.

`next`: 15 while checks pending or `mergeable` unknown (300 after 5 unknown
ticks), 60 idle, 300 paused / draft / closed.

## rss

`when: rss.py urls=<url>[,<url>…] [filter=<regex on title>] [backfill=0]`

Atom, RSS 2.0 and YouTube `videos.xml`
(`https://www.youtube.com/feeds/videos.xml?channel_id=UC…`), parsed with
regexes (no XML parser in the Monty subset): `<entry>` / `<item>` blocks,
CDATA, the five XML entities and numeric references. Ids: `yt:videoId` →
Atom `<id>` → RSS `<guid>` → link. Up to 7 feeds — one GET each inside the
8-call budget; an 8th is refused with `urls: at most 7`.

Envelope: `subject=feed/<host> events=new count=<N>` (`channel/<title>` for a
YouTube feed, `feeds/<M>` when several); one line per new entry, newest
first: `id=<id> published=<iso> title=<title> url=<url>`, then `+K more`
when the 4 KiB budget stops it. Feed text is data: tags stripped, `<`
neutralised. Per-feed failures go in the tail (`feed=<host> error=HTTP 500`
/ `error=no entries parsed (N bytes)`) and do not break the check; every
feed failing raises (`check broken`).

Per tick: `GET` each feed with `If-None-Match` / `If-Modified-Since` from
state; a `304` leaves that feed's state as is. A feed that redirected is
fetched at its final URL next time (caller headers never cross origins); a
failing feed drops its conditionals so the next check goes bare. State:
`seen` (ids newest first, FIFO ≤ 300 and ≤ 12 KB), `feeds` (`etag`,
`last_modified`, `title`, `kind`, final `url`), `seeded`.

First check (`state == {}`): every entry is seeded silently — `fire: false`,
`events=seeded count=N`, `seeded N entries from M feeds` — so the creation
dry-run says *would not fire*; `backfill=N` fires on the N newest instead. A
URL added later seeds the same way while the others keep firing. `filter`
is `re.search` on the title; non-matching entries are ignored entirely.

`next`: `Cache-Control: max-age` or `<sy:updatePeriod>` / `updateFrequency`
(the smallest across feeds), else `None`; the gate clamps it.

`probe`: one bare GET per feed → `<host>: <N> entries, parsed as
<atom|rss|youtube>` / `<host>: no entries parsed (N bytes)` / `<host>: HTTP
<status>`, joined by `; `. Never raises.

## Tests

```
python3 -m unittest discover -s assets/sensors/tests
```

`tests/monty_shim.py` `exec`s a sensor under CPython with a scripted
`http_request`, so the same file is tested unchanged.
