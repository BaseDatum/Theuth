#!/usr/bin/env python3
"""board-check — the machine read-back of a board (design 71 §1 M12, §5 row 9).

Reads a board's markdown (`doc_read(format="markdown")`, the derived `documents/<id>/note.md`,
or a committed fixture) and checks, against the working tree under the frontmatter `root:`:

  M2   `##` headings are a subset of the fixed order, never reordered, no seventh name
  M12  every `Excerpt.text` is verbatim the file's lines `start..start+n-1`
  M14  every `Graph` node and `Sequence` step carries a `ref`
  §2.3 every anchor — `code:` link, `ref`, `Excerpt` range — resolves: the file exists under root
       and the range lies inside it (`block:<id>` needs the id in the text; else unverifiable)
  M3   ≤ 3 `Excerpt`s per section; paragraphs of "How it works" without an anchor are reported
  M1   a section with two `Graph`/`Sequence` blocks is reported

Exit 1 on any FAIL. `--reference` also fails a board with < 4 sections or < 3 Excerpts (the §5 row 9
threshold for the three reference boards). `--json` prints one object per board for tabulation.

    scripts/board-check.py [--root DIR] [--reference] [--json] FILE...
"""
import argparse
import json
import os
import re
import sys

ORDER = ["What and why", "Map", "How it works", "Decisions and trade-offs", "Checks", "Open questions"]
LINK = re.compile(r"\]\(code:([^)\s]+)\)")
ANCHOR = re.compile(r"^([^#]+?)(?:#L(\d+)(?:-L(\d+))?)?$")
FENCE_OPEN = re.compile(r"^ {0,3}(`{3,}|~{3,})(.*)$")
HEADING = re.compile(r"^(#{1,6}) (.*)$")
DIAGRAMS = ("Graph", "Sequence")


def rust_lines(text):
    """Rust `str::lines`: split on \\n, strip one trailing \\r, no phantom last line."""
    out = [l[:-1] if l.endswith("\r") else l for l in text.split("\n")]
    if out and out[-1] == "":
        out.pop()
    return out


def parse_anchor(s):
    """`path#La-Lb` | `path#La` | `path` | `block:<id>` → dict, or None when malformed."""
    if s.startswith("block:"):
        return {"block": s[6:]} if len(s) > 6 else None
    m = ANCHOR.match(s)
    if not m or not m.group(1):
        return None
    path, a, b = m.group(1), m.group(2), m.group(3)
    if path.startswith(("/", "\\")) or ":" in path or any(seg in ("", ".", "..") for seg in path.split("/")):
        return None
    lines = None
    if a:
        a = int(a)
        b = int(b) if b else a
        if a < 1 or b < a:
            return None
        lines = (a, b)
    return {"path": path, "lines": lines}


def load_spec(body):
    """JSON with the three slips the engine's jsonrepair pass forgives and models actually make: a raw
    newline inside a string (`strict=False`), a backslash before a non-escape character (`\\``), and
    single-quoted strings (a Python repr pasted as JSON)."""
    try:
        return json.loads(body, strict=False)
    except ValueError:
        pass
    out, i, quote = [], 0, None          # quote: the delimiter of the string we are inside, or None
    while i < len(body):
        c = body[i]
        if quote:
            if c == "\\" and i + 1 < len(body):
                nxt = body[i + 1]
                if nxt == "'" and quote == "'":
                    out.append("'")
                elif nxt in '"\\/bfnrtu':
                    out.append(c + nxt)
                else:
                    out.append(nxt)          # `\`` → `` ` ``
                i += 2
                continue
            if c == quote:
                out.append('"'); quote = None
            elif c == '"':
                out.append('\\"')
            else:
                out.append(c)
        elif c in "\"'":
            quote = c; out.append('"')
        else:
            out.append(c)
        i += 1
    return json.loads("".join(out), strict=False)


def walk(md):
    """Yield ('fm', text) | ('heading', level, text) | ('fence', lang, body) | ('para', text)."""
    lines = md.split("\n")
    i = 0
    if lines and lines[0].strip() == "---":
        j = 1
        while j < len(lines) and lines[j].strip() != "---":
            j += 1
        yield ("fm", "\n".join(lines[1:j]))
        i = j + 1
    para = []

    def flush():
        nonlocal para
        if para:
            yield ("para", "\n".join(para))
            para = []

    while i < len(lines):
        line = lines[i]
        m = FENCE_OPEN.match(line)
        if m and not ("`" in m.group(1) and "`" in m.group(2)):
            yield from flush()
            marker, lang = m.group(1), m.group(2).strip()
            body = []
            i += 1
            while i < len(lines):
                l = lines[i].lstrip(" ") if len(lines[i]) - len(lines[i].lstrip(" ")) <= 3 else lines[i]
                run = len(l) - len(l.lstrip(marker[0]))
                if run >= len(marker) and l[run:].strip() == "":
                    break
                body.append(lines[i])
                i += 1
            yield ("fence", lang, "\n".join(body))
            i += 1
            continue
        h = HEADING.match(line)
        if h:
            yield from flush()
            yield ("heading", len(h.group(1)), h.group(2).strip())
        elif line.strip() == "":
            yield from flush()
        else:
            para.append(line)
        i += 1
    yield from flush()


def check(path, root_override=None, reference=False):
    md = open(path, encoding="utf-8").read()
    r = {"file": path, "title": None, "root": root_override, "sections": [], "fails": [], "warns": [],
         "excerpts": 0, "verbatim": 0, "diagrams": 0, "units": 0, "units_with_ref": 0,
         "anchors": 0, "resolvable": 0, "hiw_paragraphs": 0, "hiw_anchored": 0}
    fail, warn = r["fails"].append, r["warns"].append
    section = None            # current `##` name
    per_section = {}          # name → {"excerpts": n, "diagrams": n}
    anchors = []              # (anchor string, where)
    block_ids = set(re.findall(r"<!-- ([0-9a-f-]{8,}) -->", md))

    for ev in walk(md):
        kind = ev[0]
        if kind == "fm":
            m = re.search(r"^root:\s*(.+)$", ev[1], re.M)
            if m and not root_override:
                r["root"] = json.loads(m.group(1)) if m.group(1).startswith('"') else m.group(1).strip()
        elif kind == "heading":
            level, text = ev[1], ev[2]
            if level == 1 and r["title"] is None:
                r["title"] = text
            elif level == 2:
                section = text
                r["sections"].append(text)
                per_section.setdefault(text, {"excerpts": 0, "diagrams": 0})
        elif kind == "para":
            found = LINK.findall(ev[1])
            for a in found:
                anchors.append((a, f"link in {section or 'preamble'}"))
            if section == "How it works":
                r["hiw_paragraphs"] += 1
                r["hiw_anchored"] += 1 if found else 0
        elif kind == "fence" and ev[1] == "json-render":
            try:
                spec = load_spec(ev[2])
                elements = spec["elements"].values()
            except (ValueError, KeyError, AttributeError) as e:
                fail(f"json-render block in {section or 'preamble'} is not a spec: {e}")
                continue
            sec = per_section.setdefault(section, {"excerpts": 0, "diagrams": 0})
            for el in elements:
                t, p = el.get("type"), el.get("props", {})
                if t == "Excerpt":
                    r["excerpts"] += 1
                    sec["excerpts"] += 1
                    n = len(rust_lines(p.get("text", "")))
                    start = int(p.get("start", 1))
                    a = f'{p.get("path", "")}#L{start}-L{start + n - 1}'
                    anchors.append((a, "Excerpt"))
                    ok, why = verbatim(r["root"], p.get("path", ""), start, p.get("text", ""))
                    if ok:
                        r["verbatim"] += 1
                    else:
                        fail(f"Excerpt {a} not verbatim: {why}")
                elif t in DIAGRAMS:
                    r["diagrams"] += 1
                    sec["diagrams"] += 1
                    units = p.get("nodes", []) if t == "Graph" else p.get("steps", [])
                    for u in units:
                        r["units"] += 1
                        label = u.get("id") if t == "Graph" else f'step {units.index(u) + 1} ({u.get("label", "")})'
                        if u.get("ref"):
                            r["units_with_ref"] += 1
                            anchors.append((u["ref"], f"{t} {label}"))
                        else:
                            fail(f"{t} {label} in {section or 'preamble'} has no ref")

    # M2 — order
    known = [s for s in r["sections"] if s in ORDER]
    for s in r["sections"]:
        if s not in ORDER:
            fail(f"section '{s}' is not one of the six names")
    if len(set(known)) != len(known):
        fail("a section name repeats")
    if [ORDER.index(s) for s in known] != sorted(ORDER.index(s) for s in known):
        fail(f"sections out of order: {known}")

    # M3 / M1 per section
    for name, c in per_section.items():
        if c["excerpts"] > 3:
            fail(f"section '{name}' holds {c['excerpts']} Excerpts (max 3)")
        if c["diagrams"] > 1:
            warn(f"section '{name}' holds {c['diagrams']} diagrams — one shape per idea (M1)")

    # §2.3 — every anchor resolves
    if not r["root"]:
        fail("no root: frontmatter `root:` missing and --root not given")
    for a, where in anchors:
        r["anchors"] += 1
        parsed = parse_anchor(a)
        if parsed is None:
            fail(f"{where}: anchor '{a}' does not parse")
            continue
        if "block" in parsed:
            if not block_ids:
                warn(f"{where}: block:{parsed['block']} cannot be verified (no block ids in this export)")
                r["resolvable"] += 1
            elif parsed["block"] in block_ids:
                r["resolvable"] += 1
            else:
                fail(f"{where}: block:{parsed['block']} is not a block of this board")
            continue
        if not r["root"]:
            continue
        fp = os.path.join(r["root"], parsed["path"])
        if not os.path.isfile(fp):
            fail(f"{where}: {a} — file missing")
            continue
        if parsed["lines"]:
            n = len(rust_lines(open(fp, encoding="utf-8", errors="replace").read()))
            if parsed["lines"][1] > n:
                fail(f"{where}: {a} — file has {n} lines")
                continue
        r["resolvable"] += 1

    if r["hiw_paragraphs"] and r["hiw_anchored"] < r["hiw_paragraphs"]:
        warn(f"How it works: {r['hiw_paragraphs'] - r['hiw_anchored']} of {r['hiw_paragraphs']} paragraphs carry no code: link (M3)")
    if reference:
        if len(r["sections"]) < 4:
            fail(f"reference board needs ≥ 4 sections, has {len(r['sections'])}")
        if r["excerpts"] < 3:
            fail(f"reference board needs ≥ 3 Excerpts, has {r['excerpts']}")
    r["ok"] = not r["fails"]
    return r


def verbatim(root, rel, start, text):
    if not root:
        return False, "no root"
    fp = os.path.join(root, rel)
    if not os.path.isfile(fp):
        return False, "file missing"
    want = rust_lines(text)
    have = rust_lines(open(fp, encoding="utf-8", errors="replace").read())[start - 1:start - 1 + len(want)]
    if have == want:
        return True, ""
    for i, (w, h) in enumerate(zip(want, have)):
        if w != h:
            return False, f"L{start + i}: quoted {w!r} vs file {h!r}"
    return False, f"file ends after {len(have)} of {len(want)} lines"


def summary(r):
    pct = 100 * r["verbatim"] // r["excerpts"] if r["excerpts"] else 100
    return (f"sections {len(r['sections'])} · Excerpts {r['verbatim']}/{r['excerpts']} verbatim ({pct} %) · "
            f"diagrams {r['diagrams']} · units {r['units_with_ref']}/{r['units']} with ref · "
            f"anchors {r['resolvable']}/{r['anchors']} resolvable · How it works {r['hiw_anchored']}/{r['hiw_paragraphs']} paragraphs anchored")


def self_test():
    """`--self-test`: a two-line repo and a board that passes, then the three ways it fails."""
    import tempfile
    d = tempfile.mkdtemp()
    os.makedirs(os.path.join(d, "src"))
    open(os.path.join(d, "src/a.rs"), "w").write("fn a() {}\nfn b() {}\n")
    ex = '```json-render\n{"root":"e","elements":{"e":{"type":"Excerpt","props":{"path":"src/a.rs","start":%d,"text":"%s"}}}}\n```'
    g = '```json-render\n{"root":"g","elements":{"g":{"type":"Graph","props":{"nodes":[{"id":"a","label":"a"%s}],"edges":[]}}}}\n```'
    good = (f"---\nroot: {d}\n---\n# T\n\n## What and why\n\nx\n\n## Map\n\n" + g % ', "ref": "src/a.rs#L1"' +
            "\n\n## How it works\n\n[a](code:src/a.rs#L1-L2) does it.\n\n" + ex % (1, "fn a() {}\\nfn b() {}") +
            "\n\n## Checks\n\nok\n")
    def run(md, reference=False):
        p = os.path.join(d, "b.md"); open(p, "w").write(md); return check(p, None, reference)
    r = run(good); assert r["ok"], r["fails"]
    assert (r["excerpts"], r["verbatim"], r["units_with_ref"], r["anchors"], r["resolvable"]) == (1, 1, 1, 3, 3), r
    assert not run(good.replace("fn b() {}", "fn c() {}"))["ok"], "retyped excerpt must fail"
    assert not run(good.replace(', "ref": "src/a.rs#L1"', ""))["ok"], "unit without ref must fail"
    assert not run(good.replace("## Map", "## Checks", 1))["ok"], "section order must fail"
    assert not run(good.replace("#L1-L2", "#L1-L9"))["ok"], "range past EOF must fail"
    assert run(good, reference=True)["fails"] == ["reference board needs ≥ 3 Excerpts, has 1"], "reference thresholds"
    print("self-test ok")


def main():
    if sys.argv[1:] == ["--self-test"]:
        return self_test()
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("files", nargs="+")
    ap.add_argument("--root", help="override the frontmatter root (a fixture drawn on another checkout)")
    ap.add_argument("--reference", action="store_true", help="also require ≥ 4 sections and ≥ 3 Excerpts")
    ap.add_argument("--json", action="store_true")
    args = ap.parse_args()
    bad = 0
    for f in args.files:
        r = check(f, args.root, args.reference)
        bad += not r["ok"]
        if args.json:
            print(json.dumps(r, ensure_ascii=False))
            continue
        print(f"{'PASS' if r['ok'] else 'FAIL'} {f}: {r['title'] or '(untitled)'}")
        print("  " + summary(r))
        for w in r["warns"]:
            print("  warn: " + w)
        for x in r["fails"]:
            print("  FAIL: " + x)
    sys.exit(1 if bad else 0)


if __name__ == "__main__":
    main()
