
## Ideas from the literature, and hooks (design 159 §10)

Any study can seed and refresh its idea backlog from papers and the web, with every idea cited and
checked, and can run the user's own scripts around each experiment. The study session never gets
web access: the plugin searches, reads and verifies; the session sees quoted ideas as data.

### Research at setup

When the goal has prior work (an algorithm, a known optimisation, a well-studied benchmark), spend
one or two `search_web` calls on it during the interview — you have the user's tools, the study
will not. Pass what you find in the spec:

```yaml
calls: [search_web, read_url]            # the plugin may call only tools listed here, and only read-only ones
literature:
  seed:                                  # ≤ 12; each is kept only if `quote` occurs on the page at `url`
    - {text: "Detect natural runs and merge them, as Timsort does", category: algorithm,
       url: "https://en.wikipedia.org/wiki/Timsort", title: Timsort,
       quote: "Timsort is a hybrid, stable sorting algorithm"}
  max_queries: 12                        # searches per study at plateaus (default 12)
  every: 0                               # also consult every N keeps; 0 = plateaus only
  via: ""                                # a read-only MCP literature tool from `calls` instead of search_web
```

Rules for a seed: `text` is one concrete, tryable sentence; `quote` is a verbatim span of at most
300 characters you copied from the page (not a paraphrase — the plugin drops an idea whose quote it
cannot find on the page); `url` is the page you read. Never seed from memory without a page.

At each plateau the plugin writes three queries from the goal, the playbook and the dead ends,
searches (`search_web … read: 3`, charged to the study session, no model money), extracts at most
five ideas with one side call, verifies every quote against the fetched text, and stores the
survivors as `literature` ideas with a `citations` row. The brief lists them as
`[paper] <idea> — “<quote>” (<title>, <host>)` inside a data fence. `literature: {}` turns the
block on with the defaults; the draft card then says how many searches it may run.

### Hooks (before / after each experiment)

`hooks` runs the user's scripts plugin-side — compatible with pi-autoresearch's hooks:

```yaml
hooks:
  before: {command: bash hooks/before.sh, timeout: 30s}   # at experiment_begin, in the study worktree, before the brief
  after:  {command: bash hooks/after.sh,  timeout: 30s}   # when the experiment closes
evaluator_paths: [hooks/]                                 # hook scripts are evaluator files: custody copies run, never the worktree's
```

Each hook gets one JSON line in `AUTORESEARCH_HOOK_JSON` (`event`, `n`, `study` {slug, goal,
metric, incumbent, keeps}, `last` {n, state, idea, delta_pct}) plus `AUTORESEARCH_HOOK`
(`before` | `after`); `after` also gets `AUTORESEARCH_VERDICT` (`kept`, `discarded`, …) and
`AUTORESEARCH_DELTA_PCT`. The first 8 KiB of a hook's output becomes the brief section "From your
hooks" (fenced as data) — `after` output lands in the next brief. A non-zero exit or a timeout is
one incident line there, never a crash. A hook script that is not in custody is refused: hook
scripts are repo files listed in `evaluator_paths`.

Out of scope: web access inside the session, reading whole papers, literature for world studies.
