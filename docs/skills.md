# Skills

A skill is a markdown file the model can pull into context on demand. The
system prompt lists every skill's name and one-line description; the model
calls the `load_skill` tool with a name to receive the full body.

## Location

`<data_dir>/skills/**/*.md`, scanned recursively at startup. `<data_dir>` is
`--data-dir` / `WTH_DATA_DIR`, else the platform data dir (`~/Library/Application
Support/wth` on macOS, `~/.local/share/wth` on Linux). Subdirectories are for
your own organisation only; they do not affect names. Files with any other
extension are ignored. A missing directory means no skills.

## Front matter

Optional YAML between `---` delimiters at the top of the file:

| key           | default                                  |
|---------------|------------------------------------------|
| `name`        | file stem (`review.md` → `review`)       |
| `description` | first non-empty line of the body         |

The body is everything after the front matter. A file with no front matter is
still a skill. Duplicate names: the last file wins (paths sorted), with a warning
in the log.

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

Resulting system prompt fragment:

```
## Skills
Call `load_skill` with a name to get full instructions.
- commit: Write a conventional commit message for staged changes
```
