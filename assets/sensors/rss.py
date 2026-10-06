# theuth-sensor: rss 1
# arg: urls     required url[,url] — feed URLs, comma-separated (Atom, RSS 2.0, YouTube videos.xml); up to 7
# arg: filter   optional -         — regex; only entries whose title matches count
# arg: backfill optional 0         — fire on the N newest entries at the first check instead of seeding silently
"""New entries in one or more feeds (design 108 §10).

when: rss.py urls=<url>[,<url>…] [filter=<regex on title>] [backfill=0]

Everything this sensor fetches (titles, ids, links) is text written by whoever
publishes the feed. It is copied into the payload as data, never as an
instruction (40 §7): tags are stripped and `<` is neutralised.

Atom, RSS 2.0 and YouTube `videos.xml` are parsed with regular expressions —
the Monty subset has no XML parser. State: a seen-id FIFO plus per-feed
ETag / Last-Modified for conditional GETs. The first check seeds every entry
silently (`fire: false`) unless `backfill=N` asks for the N newest.

Monty subset: no classes, no generators, no file or process access. Tools are
injected globals; only `http_request` is used.
"""
import re

MAX_FEEDS = 7
# ponytail: ids are stored verbatim, so 300 ids / 12 KB is the ceiling under the
# 16 KiB state cap; feeds whose tail falls out of the FIFO re-fire it. Hash the
# ids (no hashlib in Monty — an FNV loop) if a 300+ entry feed ever matters.
SEEN_MAX = 300
SEEN_BYTES = 12000
PAYLOAD_BYTES = 4096
TAIL_BYTES = 600
ID_CHARS = 120
TITLE_CHARS = 200
URL_CHARS = 300
ENTITIES = {"amp": "&", "lt": "<", "gt": ">", "quot": '"', "apos": "'"}
MONTHS = ["jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct", "nov", "dec"]
PERIODS = {"hourly": 3600, "daily": 86400, "weekly": 604800, "monthly": 2592000, "yearly": 31536000}


def _urls(args: dict) -> list:
    out = []
    for u in str(args.get("urls") or "").split(","):
        if u.strip():
            out.append(u.strip())
    return out


def _bad_args(args: dict) -> str:
    urls = _urls(args)
    if not urls:
        return "urls= is required"
    if len(urls) > MAX_FEEDS:
        return f"urls: at most {MAX_FEEDS}"
    for u in urls:
        if re.match(r"^https?://[^\s/?#]+", u) is None:
            return f"urls: {u} is not an http(s) URL"
    if re.match(r"^[0-9]*$", str(args.get("backfill") or "")) is None:
        return "backfill= must be a number"
    try:
        re.search(str(args.get("filter") or ""), "")
    except Exception:
        return "filter= is not a valid regex"
    return ""


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


def _host(url: str) -> str:
    m = re.match(r"^https?://([^/?#]+)", url)
    return "" if m is None else m.group(1).lower()


def _header(headers: dict, name: str) -> str:
    for k in headers:
        if str(k).lower() == name:
            return str(headers[k] or "")
    return ""


async def _fetch(url: str, headers: dict) -> dict:
    """GET one feed; never raises. {"status", "url", "headers", "body", "error"}.

    Under Monty a non-2xx reply *raises* RuntimeError (README `http_request`),
    so a 304 is recognised from the message text; the CPython test shim
    returns the status instead, and both paths land here.
    """
    not_modified = {"status": 304, "url": url, "headers": {}, "body": "", "error": ""}
    try:
        reply = await http_request(url, "GET", None, headers)
    except RuntimeError as e:
        msg = " ".join(str(e).split())
        if "HTTP 304 " in msg:
            return not_modified
        return {"status": 0, "url": url, "headers": {}, "body": "", "error": msg[:120]}
    status = int(reply.get("status") or 0)
    if status == 304:
        return not_modified
    error = ""
    if status < 200 or status >= 300:
        error = f"HTTP {status}"
    elif reply.get("truncated"):
        error = "body over 1 MiB"
    body = reply.get("body")
    text = body if isinstance(body, str) else ""
    return {
        "status": status,
        "url": str(reply.get("url") or url),
        "headers": reply.get("headers") or {},
        "body": text,
        "error": error,
    }


def _cond_headers(meta: dict) -> dict:
    h: dict = {}
    if meta.get("etag"):
        h["If-None-Match"] = str(meta["etag"])
    if meta.get("last_modified"):
        h["If-Modified-Since"] = str(meta["last_modified"])
    return h


def _chr(n: int) -> str:
    if n <= 0 or n > 1114111 or (n >= 55296 and n <= 57343):
        return ""
    return chr(n)


def _unescape(s: str) -> str:
    """The five XML entities plus `&#NNN;` / `&#xHH;`; anything else stays."""
    if "&" not in s:
        return s
    parts = s.split("&")
    out = [parts[0]]
    for p in parts[1:]:
        m = re.match(r"^(#[xX][0-9A-Fa-f]{1,6}|#[0-9]{1,7}|[a-z]{1,8});", p)
        if m is None:
            out.append("&" + p)
            continue
        ent = m.group(1)
        rest = p[len(ent) + 1:]
        if ent in ENTITIES:
            out.append(ENTITIES[ent] + rest)
        elif ent.startswith("#x") or ent.startswith("#X"):
            out.append(_chr(int(ent[2:], 16)) + rest)
        elif ent.startswith("#"):
            out.append(_chr(int(ent[1:])) + rest)
        else:
            out.append("&" + p)
    return "".join(out)


def _text(raw) -> str:
    """CDATA unwrapped, tags stripped, entities decoded, one line, `<` neutralised."""
    s = str(raw or "").replace("<![CDATA[", "").replace("]]>", "")
    s = re.sub(r"(?s)<[^>]*>", "", s)
    s = re.sub(r"(?s)<[^>]*>", "", _unescape(s))
    return " ".join(s.split()).replace("<", "&lt;")


def _tag(block: str, name: str) -> str:
    """Inner text of the first `<name …>…</name>`; self-closing or absent → ""."""
    m = re.search("(?is)<" + name + r"(?:\s[^>]*[^/>])?>(.*?)</" + name + r"\s*>", block)
    return "" if m is None else m.group(1)


def _atom_link(block: str) -> str:
    first = ""
    for tag in re.findall(r"(?is)<link\b[^>]*>", block):
        href = re.search(r"""(?i)\bhref\s*=\s*["']([^"']*)""", tag)
        if href is None:
            continue
        rel = re.search(r"""(?i)\brel\s*=\s*["']([^"']*)""", tag)
        if rel is None or rel.group(1).lower() == "alternate":
            return href.group(1)
        if not first:
            first = href.group(1)
    return first


def _iso(s: str) -> str:
    """RFC 822 `Tue, 10 Jun 2003 04:00:00 GMT` → `2003-06-10T04:00:00`; anything else as is."""
    m = re.search(r"(\d{1,2}) ([A-Za-z]{3}) (\d{4}) (\d{2}):(\d{2})(?::(\d{2}))?", s)
    if m is None:
        return s
    mon = m.group(2).lower()
    if mon not in MONTHS:
        return s
    mm = ("0" + str(MONTHS.index(mon) + 1))[-2:]
    dd = ("0" + m.group(1))[-2:]
    sec = m.group(6) or "00"
    return m.group(3) + "-" + mm + "-" + dd + "T" + m.group(4) + ":" + m.group(5) + ":" + sec


def _entry(block: str, kind: str) -> dict:
    if kind == "rss":
        link = _text(_tag(block, "link"))
        ident = _text(_tag(block, "guid")) or link
        published = _tag(block, "pubDate")
    else:
        link = _text(_atom_link(block))
        ident = _text(_tag(block, "yt:videoId")) or _text(_tag(block, "id")) or link
        published = _tag(block, "published") or _tag(block, "updated")
    title = _text(_tag(block, "title"))
    return {
        "id": (ident or title)[:ID_CHARS],
        "title": title[:TITLE_CHARS],
        "url": link[:URL_CHARS],
        "published": _iso(_text(published))[:40],
    }


def _parse(body: str) -> tuple:
    """(kind, entries) with kind in atom | youtube | rss | none."""
    blocks = re.findall(r"(?is)<entry(?:\s[^>]*)?>.*?</entry>", body)
    kind = "atom"
    if blocks:
        if "<yt:videoId>" in body:
            kind = "youtube"
    else:
        blocks = re.findall(r"(?is)<item(?:\s[^>]*)?>.*?</item>", body)
        kind = "rss"
    entries = []
    for b in blocks:
        e = _entry(b, kind)
        if e["id"]:
            entries.append(e)
    if not entries:
        kind = "none"
    return kind, entries


def _feed_title(body: str) -> str:
    m = re.search(r"(?is)^(.*?)<(?:entry|item)[\s>]", body)
    head = body if m is None else m.group(1)
    return _text(_tag(head, "title"))[:80]


def _hints(headers: dict, body: str) -> list:
    """Seconds until the publisher expects a change: Cache-Control max-age, sy:updatePeriod."""
    out = []
    m = re.search(r"(?i)max-age=(\d+)", _header(headers, "cache-control"))
    if m is not None:
        out.append(int(m.group(1)))
    period = _text(_tag(body, "sy:updatePeriod")).lower()
    if period in PERIODS:
        freq = _text(_tag(body, "sy:updateFrequency"))
        n = int(freq) if re.match(r"^[1-9][0-9]*$", freq) is not None else 1
        out.append(PERIODS[period] // n)
    return out


def _newest_first(entries: list) -> list:
    # ponytail: lexical sort on the published string (ISO or converted RFC 822,
    # offsets ignored); feeds mixing formats interleave roughly, not exactly.
    keys = []
    for i, e in enumerate(entries):
        keys.append(e["published"] + " " + str(999999 - i))
    out = []
    for k in reversed(sorted(keys)):
        out.append(entries[999999 - int(k.split(" ")[-1])])
    return out


def _fit_seen(seen: list) -> list:
    out = seen[:SEEN_MAX]
    used = 0
    n = 0
    for s in out:
        used += _utf8_len(s) + 4
        if used > SEEN_BYTES:
            break
        n += 1
    return out[:n]


def _token(s: str) -> str:
    return re.sub(r"\s+", "_", s.strip())[:40] or "untitled"


def _subject(urls: list, feeds: dict) -> str:
    if len(urls) > 1:
        return f"feeds/{len(urls)}"
    meta = feeds.get(urls[0]) or {}
    if meta.get("kind") == "youtube" and meta.get("title"):
        return "channel/" + _token(str(meta["title"]))
    return "feed/" + _host(urls[0])


def _payload(first_line: str, lines: list, tail: list) -> str:
    tail_text = _clip("\n".join(tail), TAIL_BYTES) if tail else ""
    budget = PAYLOAD_BYTES - _utf8_len(first_line) - _utf8_len(tail_text) - 24
    out = [first_line]
    used = 0
    shown = 0
    for ln in lines:
        n = _utf8_len(ln) + 1
        if used + n > budget:
            break
        out.append(ln)
        used += n
        shown += 1
    if shown < len(lines):
        out.append(f"+{len(lines) - shown} more")
    if tail_text:
        out.append(tail_text)
    return "\n".join(out)


async def check(state: dict, args: dict) -> dict:
    bad = _bad_args(args)
    if bad:
        raise ValueError(bad)
    urls = _urls(args)
    pattern = str(args.get("filter") or "")
    backfill = int(str(args.get("backfill") or "0"))
    first = len(state) == 0
    seen_prev = [str(x) for x in (state.get("seen") or [])]
    seen_set = set(seen_prev)
    feeds_prev = state.get("feeds") or {}
    feeds: dict = {}
    fresh: list = []
    tail: list = []
    hints: list = []
    failed = 0

    for url in urls:
        meta = feeds_prev.get(url) or {}
        host = _host(url)
        r = await _fetch(str(meta.get("url") or url), _cond_headers(meta))
        if r["status"] == 304:
            feeds[url] = meta
            continue
        if r["error"]:
            failed += 1
            tail.append(f"feed={host} error={r['error']}")
            if url in feeds_prev:
                # Drop the conditionals and the redirect target: the next check goes bare.
                feeds[url] = {"title": str(meta.get("title") or ""), "kind": str(meta.get("kind") or "")}
            continue
        body = r["body"]
        kind, entries = _parse(body)
        for h in _hints(r["headers"], body):
            hints.append(h)
        new_meta: dict = {
            "etag": _header(r["headers"], "etag")[:200],
            "last_modified": _header(r["headers"], "last-modified")[:64],
            "title": _feed_title(body),
            "kind": kind,
        }
        if r["url"] != url:
            new_meta["url"] = r["url"][:URL_CHARS]
        feeds[url] = new_meta
        if not entries:
            tail.append(f"feed={host} error=no entries parsed ({_utf8_len(body)} bytes)")
            continue
        seeding = url not in feeds_prev
        for e in entries:
            if pattern and re.search(pattern, e["title"]) is None:
                continue
            if e["id"] in seen_set:
                continue
            seen_set.add(e["id"])
            e["seed"] = seeding
            e["host"] = host
            fresh.append(e)

    if failed == len(urls):
        raise RuntimeError("every feed failed: " + "; ".join(tail))

    ordered = _newest_first(fresh)
    if first:
        for i, e in enumerate(ordered):
            e["seed"] = i >= backfill
    new = [e for e in ordered if not e["seed"]]
    seeded = [e for e in ordered if e["seed"]]
    seen = _fit_seen([e["id"] for e in new] + [e["id"] for e in seeded] + seen_prev)

    if new:
        events = "new"
        count = len(new)
    elif seeded:
        events = "seeded"
        count = len(seeded)
    else:
        events = ""
        count = 0
    first_line = f"subject={_subject(urls, feeds)} events={events} count={count}"
    lines = [f"id={e['id']} published={e['published']} title={e['title']} url={e['url']}" for e in new]
    if seeded:
        hosts = set([e["host"] for e in seeded])
        tail = [f"seeded {len(seeded)} entries from {len(hosts)} feeds"] + tail
    nxt = min(hints) if hints else None
    return {
        "fire": len(new) > 0,
        "payload": _payload(first_line, lines, tail),
        "state": {"seen": seen, "feeds": feeds, "seeded": True},
        "next": nxt,
    }


async def probe(args: dict) -> str | None:
    bad = _bad_args(args)
    if bad:
        return bad
    out = []
    for url in _urls(args):
        host = _host(url)
        r = await _fetch(url, {})
        if r["error"]:
            out.append(f"{host}: {r['error']}")
            continue
        kind, entries = _parse(r["body"])
        if not entries:
            out.append(f"{host}: no entries parsed ({_utf8_len(r['body'])} bytes)")
        else:
            out.append(f"{host}: {len(entries)} entries, parsed as {kind}")
    return "; ".join(out)
