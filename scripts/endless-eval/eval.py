#!/usr/bin/env python3
"""Offline eval: single-unit vs batch endless-memory summaries. Stdlib only. See README.md."""
import argparse, json, os, re, sys, time, urllib.request, urllib.error

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
ENDLESS = os.path.join(ROOT, "crates/agent/src/endless")
FIXTURE = os.path.join(ROOT, "crates/agent/tests/fixtures/endless/B.main.jsonl")
BATCH_MAX_UNITS, BATCH_MAX_BYTES, EARLIER_MAX = 16, 24_000, 24

# ---- guards: constants are parsed from guards.rs so they cannot drift ----
_g = open(os.path.join(ENDLESS, "guards.rs"), encoding="utf-8").read()
def _block(name):
    return re.search(r"const %s: &\[.*?\] = &\[(.*?)\n\];" % name, _g, re.S).group(1)
def _unq(s):
    return json.loads(s)
PHRASES = [_unq(m) for m in re.findall(r'^\s*("(?:[^"\\]|\\.)*"),\s*$', _block("PROMPT_PHRASES"), re.M)]
STATUS = [_unq(m) for m in re.findall(r'\("[a-z]+", ("[a-z]+")\)', _block("STATUS"))]
STOP = {_unq(m) for m in re.findall(r'^\s*("(?:[^"\\]|\\.)*"),\s*$', _block("STOP"), re.M)}
assert PHRASES and STATUS and STOP, "failed to parse guards.rs"

def ruler(n):
    return ("|0123456789abcdef" * (n // 17 + 1))[:n]

def toks(s):
    out = []
    for w in re.split(r"[^\w'/.\-]", s):
        w = re.sub(r"^\W+|\W+$", "", w, flags=re.A).lower() if w else w
        if w:
            out.append(w)
    return out

def ngrams(t, n):
    return {tuple(t[i:i + n]) for i in range(len(t) - n + 1)}

def cut8(s, n):
    return s.encode()[:n].decode(errors="ignore")

def clean(line):
    s = line.strip()
    m = re.match(r"^(\d+)\+(\d+)\|", s)
    if m:
        s = s[m.end():].strip()
    return s.replace("\n", " ")

def check(line, inp, nb):
    """Mirror of guards::check; returns (cleaned, trip|None)."""
    line = clean(line)
    if not line:
        return line, "empty"
    low, rl = line.lower(), ruler(nb)
    if low.startswith("<chat>") or line.startswith(cut8(rl, 8)) or any(low.startswith(p.lower()) for p in PHRASES):
        return line, "format"
    lt = toks(line)
    six = ngrams(lt, 6)
    if six:
        for f in [rl] + PHRASES:
            if ngrams(toks(f), 6) & six:
                return line, "copy"
    if "0123456789abcdef" in low:
        return line, "copy"
    it = toks(inp)
    if not ngrams(it, 3) & ngrams(lt, 3):
        rare = lambda w: len(w) >= 4 and w not in STOP
        if len({w for w in lt if rare(w)} & {w for w in it if rare(w)}) < 2:
            return line, "grounding"
    il = inp.lower()
    for stem in STATUS:
        if any(w.startswith(stem) for w in lt) and stem not in il:
            return line, "status"
    if len(line.encode()) > nb:
        # R1 Q2 (as the engine): a size-only trip is cut at a word boundary and accepted.
        return cut_words(line, nb), None
    return line, None

def cut_words(s, nb):
    """Mirror of guards::cut_words: ≤ nb bytes, at a word boundary in the second half, ending in '…'."""
    b = s.encode()
    if len(b) <= nb:
        return s
    head = b[:nb - 3].decode("utf-8", errors="ignore")
    p = head.rfind(" ")
    if p >= len(head.encode()) // 2:
        head = head[:p]
    return head.rstrip() + "…"

# ---- key terms ----
def terms(s):
    out = set()
    for w in re.findall(r"[\w./\-]+", s):
        w = w.strip(".-/") if not w.startswith("/") else w.rstrip(".-")
        if not w:
            continue
        if "/" in w or re.search(r"[A-Za-z]\.[A-Za-z0-9]{1,5}$", w):
            out.add(w)
        elif re.fullmatch(r"\d{2,}", w):
            out.add(w)
        elif len(w) >= 3 and re.search(r"[A-Za-z]", w) and ("_" in w or "." in w or re.search(r"[a-z][A-Z]", w)):
            out.add(w)
    return out

# ---- prompts ----
def system_prompt():
    return open(os.path.join(ENDLESS, "compact_prompt.txt"), encoding="utf-8").read().replace("{agent}", "theuth")

def head(earlier, nb):
    s = "Lines already written for earlier messages of this stretch (text only, newest last):\n"
    s += "".join(l.replace("\n", " ") + "\n" for l in earlier) if earlier else "none\n"
    return s + f"\nFor scale, this ruler is exactly {nb} bytes and is NOT part of the chat:\n{ruler(nb)}\n\n"

def single_prompt(earlier, u, nb):
    return head(earlier, nb) + f"Compress this message into one line, in at most {nb} bytes:\n{u['kind']}: {u['text']}"

def batch_prompt(earlier, us, nb):
    n = len(us)
    body = "\n\n".join(f"[{k}] {u['kind']}: {u['text']}" for k, u in enumerate(us, 1))
    return head(earlier, nb) + (
        f"Compress each of these {n} messages into one line each, in at most {nb} bytes per line. "
        f"Reply with exactly {n} lines, numbered in order as `1| …`, `2| …`, one per message, nothing else:\n\n{body}")

# ---- providers ----
def call(a, key, system, user, max_tokens):
    if a.provider == "anthropic":
        url, hdr = a.base_url.rstrip("/") + "/v1/messages", {"x-api-key": key, "anthropic-version": "2023-06-01"}
        body = {"model": a.model, "max_tokens": max_tokens, "system": system, "messages": [{"role": "user", "content": user}]}
        if a.thinking == "disabled":
            # R1 Q1: Haiku 5.x thinks adaptively unless told not to; omitting the field is not enough.
            body["thinking"] = {"type": "disabled"}
    else:
        url, hdr = a.base_url.rstrip("/") + "/v1/chat/completions", {"Authorization": "Bearer " + key}
        body = {"model": a.model, "max_tokens": max_tokens,
                "messages": [{"role": "system", "content": system}, {"role": "user", "content": user}]}
    req = urllib.request.Request(url, json.dumps(body).encode(), {"content-type": "application/json", **hdr})
    try:
        with urllib.request.urlopen(req, timeout=a.timeout) as r:
            d = json.load(r)
    except urllib.error.HTTPError as e:
        raise RuntimeError(f"HTTP {e.code}: {e.read()[:200].decode(errors='replace')}")
    u = d.get("usage") or {}
    if a.provider == "anthropic":
        text = "".join(c.get("text", "") for c in d.get("content", []) if c.get("type") == "text")
        return text, u.get("input_tokens", 0), u.get("output_tokens", 0)
    return d["choices"][0]["message"]["content"] or "", u.get("prompt_tokens", 0), u.get("completion_tokens", 0)

def parse_batch(text, n):
    """Mirror of compactor::batch_lines: the last line of a repeated number wins; an
    unnumbered line continues the numbered line before it."""
    got, last = {}, None
    for l in text.splitlines():
        l = l.strip()
        if not l:
            continue
        m = re.match(r"^(\d+)\s*[|.:)]\s*(.*)$", l)
        if m:
            k = int(m.group(1))
            if 1 <= k <= n:
                got[k], last = m.group(2).strip(), k
            else:
                last = None
        elif last is not None:
            got[last] += " " + l
    return [got.get(k, "") for k in range(1, n + 1)]

# ---- run ----
def load(limit):
    us = [json.loads(l) for l in open(FIXTURE, encoding="utf-8") if l.strip()]
    return us[:limit] if limit else us

def run(a, key, units, mode):
    nb, sysp = a.node_bytes, system_cache
    st = {"calls": 0, "tin": 0, "tout": 0, "errors": 0, "lines": []}  # lines: (unit, line, trip)
    earlier = []
    def note(u, raw):
        line, trip = check(raw, f"{u['kind']}: {u['text']}", nb)
        st["lines"].append((u, line, trip))
        if trip is None:
            earlier.append(line)
    def send(prompt, mt):
        st["calls"] += 1
        try:
            t, i, o = call(a, key, sysp, prompt, mt)
        except Exception as e:
            st["errors"] += 1
            print(f"  [{mode}] call failed: {e}", file=sys.stderr)
            return None
        st["tin"] += i; st["tout"] += o
        return t
    pending = []
    def flush():
        if not pending:
            return
        us = list(pending); pending.clear()
        t = send(batch_prompt(earlier[-EARLIER_MAX:], us, nb), min(600 * len(us), 8000))
        for u, raw in zip(us, parse_batch(t, len(us)) if t is not None else [""] * len(us)):
            note(u, raw)
    for u in units:
        if len(f"{u['kind']}: {u['text']}".encode()) <= nb:
            earlier.append(f"{u['kind']}: {u['text']}")  # free: verbatim, no call
            continue
        if mode == "single":
            t = send(single_prompt(earlier[-EARLIER_MAX:], u, nb), 600)
            note(u, t or "")
        else:
            if pending and (len(pending) >= a.batch_units or
                            sum(len(p["text"].encode()) for p in pending) + len(u["text"].encode()) > BATCH_MAX_BYTES):
                flush()
            pending.append(u)
    flush()
    L = st["lines"]
    n = len(L)
    ret = [len(terms(u["text"]) & terms(l)) / len(terms(u["text"])) if l else 0.0
           for u, l, _ in L if terms(u["text"])]
    nonempty = [len(l.encode()) for _, l, _ in L if l]
    trips = {}
    for _, _, t in L:
        if t: trips[t] = trips.get(t, 0) + 1
    return {"units": n, "calls": st["calls"], "call_errors": st["errors"],
            "pass_rate": sum(t is None for _, _, t in L) / n if n else 0.0, "trips": trips,
            "mean_len": sum(nonempty) / len(nonempty) if nonempty else 0.0,
            "retention": sum(ret) / len(ret) if ret else 0.0,
            "tokens_in": st["tin"], "tokens_out": st["tout"],
            "cost_usd": (st["tin"] * a.price_in + st["tout"] * a.price_out) / 1e6,
            "lines": [{"i": u["i"], "kind": u["kind"], "line": l, "trip": t} for u, l, t in L]}

def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("--provider", choices=["anthropic", "openai"], default="anthropic")
    p.add_argument("--base-url")
    p.add_argument("--model", required=True)
    p.add_argument("--key-env", default="ANTHROPIC_API_KEY")
    p.add_argument("--limit", type=int, default=0, help="first N units (0 = all)")
    p.add_argument("--batch-units", type=int, default=BATCH_MAX_UNITS)
    p.add_argument("--node-bytes", type=int, default=512)
    p.add_argument("--price-in", type=float, default=1.0)
    p.add_argument("--price-out", type=float, default=5.0)
    p.add_argument("--timeout", type=float, default=120)
    p.add_argument("--out", default="/tmp/137g-e/eval/result.json")
    p.add_argument("--thinking", choices=["disabled", "default"], default="disabled",
                   help="anthropic: send thinking {type: disabled} (the engine's compactor does, R1 Q1)")
    a = p.parse_args()
    a.base_url = a.base_url or {"anthropic": "https://api.anthropic.com", "openai": "https://api.openai.com"}[a.provider]
    key = os.environ.get(a.key_env)
    if not key:
        sys.exit(f"env var {a.key_env} is not set; no request sent")
    global system_cache
    system_cache = system_prompt()
    units = load(a.limit)
    free = sum(len(f"{u['kind']}: {u['text']}".encode()) <= a.node_bytes for u in units)
    res = {m: run(a, key, units, m) for m in ("single", "batch")}
    print(f"units={len(units)} free(verbatim, excluded)={free} node_bytes={a.node_bytes} model={a.model} thinking={a.thinking}")
    print(f"{'mode':7}{'units':>6}{'calls':>6}{'pass%':>7}{'meanlen':>9}{'retain':>8}{'tok_in':>9}{'tok_out':>9}{'usd':>9}")
    for m, r in res.items():
        print(f"{m:7}{r['units']:>6}{r['calls']:>6}{100*r['pass_rate']:>7.1f}{r['mean_len']:>9.1f}"
              f"{r['retention']:>8.3f}{r['tokens_in']:>9}{r['tokens_out']:>9}{r['cost_usd']:>9.4f}")
    for m, r in res.items():
        if r["trips"] or r["call_errors"]:
            print(f"  {m}: trips={r['trips']} call_errors={r['call_errors']}")
    os.makedirs(os.path.dirname(os.path.abspath(a.out)), exist_ok=True)
    json.dump({"args": {k: v for k, v in vars(a).items()}, "free": free, "modes": res}, open(a.out, "w"), indent=1)
    print("wrote", a.out)

if __name__ == "__main__":
    main()
