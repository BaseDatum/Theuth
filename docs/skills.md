# Skills

A skill is markdown the model can pull into context on demand. The system
prompt lists every skill's name and one-line description; the model calls the
`load_skill` tool with a name to receive the full body. Skills marked `always`
are inlined into the prompt instead of listed.

You can hand a skill to the model yourself: `/skills` then Enter, or
`/<skill> [words]` (`/caveman lite`). The body goes in as your input — a
collapsed `◇ caveman` card, then your words — starting a turn when the model
is idle or steering the running one. Nothing is "enabled" permanently: a skill
is in context until the next compaction or `/clear`. The status line lists the
skills currently in context (`skills: caveman, ponytail`), whoever loaded them,
and the `/skills` picker marks them `●`.

## Location

`<data_dir>/skills/`, walked recursively on every model call (no restart, no
reload). `<data_dir>` is `--data-dir` / `THEUTH_DATA_DIR`, else `~/.theuth`. A
missing directory means no skills.

| path                          | is                                                         |
|-------------------------------|------------------------------------------------------------|
| `<dir>/SKILL.md`              | one skill; the rest of `<dir>` is supporting files, not walked |
| any other `*.md`              | one skill                                                  |
| `README.md`, `LICENSE.md`, `CHANGELOG.md`, `CONTRIBUTING.md` (any case) | ignored |
| `.git/`, other extensions     | ignored                                                    |

Subdirectories are for organisation only; they do not affect names.

## Front matter

Optional YAML between `---` delimiters at the top of the file:

| key           | default                                  | effect                                            |
|---------------|------------------------------------------|---------------------------------------------------|
| `name`        | file stem, or directory name for `SKILL.md` |                                                |
| `description` | first non-empty line of the body         | shown in the index                                |
| `always`      | `false`                                  | `true`: body inlined into the system prompt, not indexed |

Unknown keys (`license`, `allowed-tools`, `metadata`, …) are ignored, so
third-party `SKILL.md` repos load as they are. The body is everything after
the front matter. A file with no front matter is still a skill. Duplicate
names: the last file wins (paths sorted), with a warning in the log.

## Git repos

A skill repo is a clone directly under `skills/`; the clone is the record, no
manifest. Its skills show `[repo-name]` in `/skills` and refuse `add_skill` /
`remove_skill` (edit upstream, or `remove_skill_repo`).

```
~/.theuth/skills/
  commit.md                   loose skill
  review/SKILL.md             directory skill
  anthropics-skills/          clone (/skills add https://github.com/anthropics/skills)
    .git/
    skills/pdf/SKILL.md       indexed, repo = anthropics-skills
    README.md                 ignored
```

| TUI                  | tool                | does                                                          |
|----------------------|---------------------|---------------------------------------------------------------|
| `/skills`, `/<skill> [words]` | `load_skill` | picker: Enter hands the skill to the model, `s` shows the body. The tool also returns the relative paths of supporting files next to `SKILL.md` for `read_file` |
|                      | `add_skill`         | writes `skills/<name>.md` with `name`/`description`/`always` front matter |
| `/skills rm <name>`  | `remove_skill`      | deletes a local skill                                         |
| `/skills add <url>`  | `add_skill_repo`    | `git clone --depth 1` into `skills/<name>` (default: repo basename); returns the skills found |
|                      | `remove_skill_repo` | `rm -rf skills/<name>` if it has `.git`                       |
| `/skills sync`       | `sync_skills`       | `git pull --ff-only` in every repo; per-repo `updated` / `unchanged` / `error` |

Names are `[a-z0-9][a-z0-9._-]*`. Git is the user's `git` binary; without it
the repo operations report that and everything else works. Changes are live
on the next model call. Same operations over RPC: `list_skills`, `load_skill`,
`use_skill`, `add_skill`, `remove_skill`, `add_skill_repo`, `remove_skill_repo`,
`sync_skills`.

## Example

`<data_dir>/skills/git/commit.md`:

```markdown
---
name: commit
description: Write a conventional commit message for staged changes
---

Run `git diff --cached`, then write a message in the form
`type(scope): subject`, subject under 50 characters, body only when the
"why" is not obvious from the diff.
```

`<data_dir>/skills/house-rules.md`:

```markdown
---
always: true
---
Never run `git push`.
```

Resulting system prompt fragment:

```
Never run `git push`.

## Skills
Call `load_skill` with a name to get full instructions.
- commit: Write a conventional commit message for staged changes
```

## prompt.md

`<data_dir>/prompt.md` holds standing instructions and is read verbatim into
the system prompt on every model call, between the built-in header and the
always-on skills. Missing file: nothing added. Difference from an `always`
skill: none the model sees; `prompt.md` is the one file for "how I want you
to behave", skills are the reusable pieces. `/prompt` (TUI) and `get_prompt`
(RPC) show the assembled prompt exactly as the model receives it.
