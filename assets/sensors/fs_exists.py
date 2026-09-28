# theuth-sensor: fs_exists 2
"""Fires while a path exists under the file's cwd (design 40 §9 step 2).

when: fs_exists.py path=<glob>

`glob` is rooted at the schedule file's cwd (`*` stays within one path segment,
`**` crosses directories; a pattern without `/` matches file names anywhere
under cwd). An absolute pattern is split at its first wildcard segment into the
directory to search and the pattern under it; that directory must lie inside
cwd (the runtime refuses anything outside). The matched paths are quoted into
the payload as data, never as an instruction. State is unused; `next` is None
(the gate keeps `every`).
"""


def _split(pattern: str) -> tuple:
    """`/a/b/*.txt` → (`/a/b`, `*.txt`); `/a/b/file` → (`/a/b`, `file`); relative → (None, pattern)."""
    if not pattern.startswith("/"):
        return None, pattern
    parts = pattern.split("/")
    literal = []
    rest = []
    for i in range(1, len(parts)):
        seg = parts[i]
        if rest or ("*" in seg or "?" in seg or "[" in seg):
            rest.append(seg)
        else:
            literal.append(seg)
    if not rest:
        rest = [literal.pop()]
    return "/" + "/".join(literal), "/".join(rest)


async def check(state: dict, args: dict) -> dict:
    pattern = str(args.get("path") or "")
    if not pattern:
        raise ValueError("fs_exists: `path=<glob>` is required")
    base, rel = _split(pattern)
    if base is None:
        found = await glob(rel)
    else:
        found = await glob(rel, False, 500, base)
    files = sorted(found["files"])
    payload = "subject=fs/" + pattern + " events=exists"
    if files:
        payload = payload + "\n" + "\n".join(files)
    return {"fire": len(files) > 0, "payload": payload, "state": {}, "next": None}
