# 108 — as built, RSS worker (`rss.py`)

Branch `feat/108-rss`, base `8490bb5a`. Spec: `108-routines-ui.md` §10, §9.1; `40-triggers.md`
§1.4 / §4.3 / §5.

## Files

- `assets/sensors/rss.py` — new. Header `# theuth-sensor: rss 1` + the three `# arg:` lines
  verbatim from §10; no `# needs:`, no `# hosts:`. `import re` only.
- `assets/sensors/tests/test_rss.py` — new, 22 cases; fixtures are inline builders
  (`rss()`, `atom()`, `youtube()`, `MALFORMED`) so the 350-entry / 200-new cases are generated.
  `FakeFeeds` keys on the full URL, answers `(status, body[, headers])`, and with `raising=True`
  raises `RuntimeError("http_request: HTTP <n> for GET …")` the way the Monty bridge does.
- `assets/sensors/README.md` — new `## rss` section between `## pr_watch` and `## Tests`.
  Nothing else touched.

## Judgement calls beyond the doc

1. **Bridge-raised errors are read from the message.** Under Monty a non-2xx *raises*
   (`bridge.rs:459-465`, `http.rs:207-208`); the shim returns the status. `_fetch` handles both:
   `\bHTTP (\d{3})\b` in the message → `304` = not modified, else `error=HTTP <n>`; a message
   without a code (over-budget, redirect refused) is kept as `error=<first 120 chars>`.
   **Engine finding for ENG/orchestrator:** the fetcher treats *every* 3xx as a redirect
   (`web/src/fetch/mod.rs:279` `status.is_redirection()`), so a genuine `304 Not Modified` —
   which has no `Location` — fails as `FetchError::Failed("redirect missing Location header")`
   (`mod.rs:286-290`) and reaches the sensor as `RuntimeError("http_request: redirect missing
   Location header")`, never as `HTTP 304`. Not my crate. Workaround in `_fetch`: that message
   on a GET that *sent conditional headers* is taken as not-modified (a real redirect always
   carries `Location`); without conditionals it stays an error. Both the engine's message and
   the plain `HTTP 304` form are pinned (`test_304_raised_as_the_monty_bridge_does`). The
   proper fix is in `fetch/mod.rs`: exclude 304 from the redirect branch and return it as a
   normal non-2xx `RawResponse`.
2. **Redirects.** `fetch/mod.rs:294` refuses a cross-origin redirect when any caller header is
   present, so a conditional GET of a redirecting feed would fail every check after the first.
   The reply's final `url` is stored in `feeds[url].url` and fetched directly next time; a
   failing feed drops `etag` / `last_modified` / `url` so the following check goes bare.
   Empty `{}` headers on the first GET count as "no caller headers" (`!headers.is_empty()`).
3. **Per-feed seeding.** A URL with no `feeds` entry (added later, or whose first GET failed)
   seeds silently while the others fire; `backfill` applies only on the global first check
   (`state == {}`). Otherwise a feed that 500s on the very first check would dump its whole
   back-catalogue on the next one.
4. **Parse failure ≠ HTTP failure.** A 200 with no `<entry>`/`<item>` (HTML challenge page,
   garbage) is a tail diagnostic `error=no entries parsed (N bytes)`, `fire: false`, no raise —
   the brief's "malformed → never an exception". Only HTTP failures count toward "every feed
   failed → raise".
5. **Bad args raise `ValueError` in `check`** (pr_watch's idiom; the ENG checker test expects a
   `ValueError` reach on empty args, `sensor/mod.rs:1197-1224`) and return the string in `probe`.
   `urls: at most 7` is the 8th-URL message in both.
6. **State bound is count *and* bytes.** 300 ids × 120 chars would be 36 KiB, over the 16 KiB
   cap the doc assumes; `_fit_seen` keeps ≤ 300 ids and ≤ 12 000 bytes (sum of UTF-8 length + 4).
   Pinned by `test_state_under_16k_with_long_ids`. `ponytail:` comment names the ceiling.
7. **Summary / `media:description` is not parsed** — §10's payload line is
   `id= published= title= url=` and nothing reads a summary. Add when a consumer needs it.
8. **Text cleanup order**: CDATA unwrap → strip tags → decode entities → strip tags again →
   collapse whitespace → `<` → `&lt;`. Decoded markup (`type="html"` titles) is stripped; a
   literal `<` in prose survives neutralised.
9. **Subject tokens**: `parse_envelope` splits on whitespace (`mod.rs:618-628`), so
   `channel/<title>` collapses whitespace to `_` and is capped at 40 chars.
10. **Newest-first** is a lexical sort on the `published` string (ISO as given; RFC 822
    `pubDate` converted to `YYYY-MM-DDTHH:MM:SS`, offset dropped); ties keep feed order.
11. **`seeded N entries from M feeds`** — M = distinct feeds contributing seeded entries.
12. **304 via the shim** also keeps `kind`/`title` in `feeds` so the subject stays stable.

## Gates

```
python3 -m unittest discover -s assets/sensors/tests
Ran 60 tests in 0.307s
FAILED (failures=13)

python3 -m unittest discover -s assets/sensors/tests -p 'test_rss.py'
Ran 23 tests in 0.100s
OK
```

The 13 failures were all in `test_pr_watch.py` and **pre-existed on the base commit**: with
`rss.py` and `test_rss.py` stashed, `python3 -m unittest test_pr_watch` → `Ran 37 tests …
FAILED (failures=13)`, every one an unexpected extra `stale` event. Cause: the fixtures pinned
`updated_at` to `2026-09-28` and `pr_watch._hours_since` reads the wall clock
(`pr_watch.py:152-161`), so from 2026-10-01 on every PR was ≥ 72 h stale.

**F1 (follow-up, ownership extended to `test_pr_watch.py`):** `T0/T1/T2` are now
`iso_hours_ago(3/2/1)` (UTC `strftime`, the shape `_hours_since`'s regex reads); `test_stale_once`
keeps its 2020 instant and still asserts `stale`. `pr_watch.py` unchanged; `monty_shim.py` has no
clock hook, none needed. Gate after F1:

```
python3 -m unittest discover -s assets/sensors/tests
Ran 60 tests in 0.390s
OK
```

## Monty-subset constructs I could not verify (no engine build here)

Beyond what `pr_watch.py` already uses: `try/except RuntimeError as e` + `str(e)` (08 §2 lists
try/except and catchable host exceptions); `except Exception` around a user regex compile in
`_bad_args`; `isinstance(body, str)`; `chr()`; `int(s, 16)`; `reversed(sorted(list))`;
`min(list)`; `list.index`; `str.startswith/lower/strip`; `re.sub` with a plain string
replacement; `re.findall` with a group-free pattern (whole matches); inline `(?is)` flags,
`\b`, lazy `.*?`, `(?:…)?` on fancy-regex; a regex built by string concatenation; `[:n]` /
`[-2:]` slices; `//`; `-> tuple` + `a, b = f()` unpacking; `str | None` return annotation (as
pr_watch). Avoided on purpose: nested f-strings, walrus, generators, classes, `str.find`,
`sorted(key=…)`, format specs in f-strings, `dict.setdefault`, `xml`/`html`/`urllib`/`json`.
Variables are never rebound to a different type in one scope (ty's `invalid-assignment`,
`repl.rs:2005-2011`).

## Not exercised

- The Monty type check and a live run under the sensor profile (`cargo test -p agent sensor::`
  over `rss.py` — the integration tree's gate).
- Real feeds over the network (no network); a body cut at 1 MiB (`truncated: true` path is
  straight-line: `error=body over 1 MiB`).
- The `knowledge_search` / fs tools (unused).
