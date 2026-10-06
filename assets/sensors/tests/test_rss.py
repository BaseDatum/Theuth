"""rss.py under CPython via monty_shim. Run: python3 -m unittest discover -s assets/sensors/tests"""
import json
import re
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from monty_shim import SENSORS, load, run  # noqa: E402

BLOG = "https://blog.example/feed.xml"
ATOM = "https://atom.example/feed"
YT = "https://www.youtube.com/feeds/videos.xml?channel_id=UCabc"
BAD = "https://bad.example/feed"


class FakeFeeds:
    """Scripted responder keyed on the full URL. `routes[url] = body | (status, body) | (status, body, headers)`.

    `raising=True` mimics the Monty bridge, where a non-2xx reply raises
    RuntimeError("http_request: HTTP <status> for GET <url> (...)") instead of returning —
    except a 304, which the fetcher takes for a redirect and reports as
    `redirect missing Location header` (`web/src/fetch/mod.rs` `is_redirection`).
    """

    def __init__(self, routes, raising=False):
        self.routes = dict(routes)
        self.raising = raising
        self.calls = []

    async def __call__(self, url, method, body=None, headers=None):
        self.calls.append((method, url, body, headers))
        hit = self.routes.get(url)
        status, reply, resp_headers = 200, hit, {}
        if hit is None:
            status, reply = 404, "not found"
        elif isinstance(hit, tuple):
            status, reply = hit[0], hit[1]
            if len(hit) > 2:
                resp_headers = hit[2]
        if self.raising and status == 304:
            raise RuntimeError("http_request: redirect missing Location header")
        if self.raising and not 200 <= status < 300:
            raise RuntimeError(f"http_request: HTTP {status} for {method} {url} (0 bytes, 1 ms)")
        return {"url": url, "status": status, "headers": resp_headers, "body": reply, "truncated": False, "took_ms": 1}


def rss(items):
    """RSS 2.0 document; items = [(guid, title, link, pubDate)]."""
    body = "".join(
        f"<item><title>{t}</title><link>{l}</link><guid isPermaLink=\"false\">{g}</guid><pubDate>{d}</pubDate></item>"
        for g, t, l, d in items
    )
    return (
        '<?xml version="1.0"?><rss version="2.0" xmlns:sy="http://purl.org/rss/1.0/modules/syndication/">'
        "<channel><title>Example &amp; Blog</title><link>https://blog.example/</link>"
        f"{body}</channel></rss>"
    )


def atom(entries):
    """Atom document; entries = [(id, title, href, published)]."""
    body = "".join(
        f'<entry><id>{i}</id><title type="text">{t}</title><link rel="self" href="https://atom.example/self"/>'
        f'<link rel="alternate" href="{h}"/><published>{p}</published><updated>{p}</updated></entry>'
        for i, t, h, p in entries
    )
    return f'<?xml version="1.0"?><feed xmlns="http://www.w3.org/2005/Atom"><title>Atom Feed</title>{body}</feed>'


def youtube(videos, channel="Cool Channel"):
    """YouTube videos.xml; videos = [(videoId, title, published)]."""
    body = "".join(
        f"<entry><id>yt:video:{v}</id><yt:videoId>{v}</yt:videoId><yt:channelId>UCabc</yt:channelId>"
        f"<title>{t}</title><link rel=\"alternate\" href=\"https://www.youtube.com/watch?v={v}\"/>"
        f"<published>{p}</published><media:group><media:title>{t}</media:title>"
        f"<media:description>desc {v}</media:description></media:group></entry>"
        for v, t, p in videos
    )
    return (
        '<?xml version="1.0"?><feed xmlns:yt="http://www.youtube.com/xml/schemas/2015" '
        'xmlns:media="http://search.yahoo.com/mrss/" xmlns="http://www.w3.org/2005/Atom">'
        f"<title>{channel}</title>{body}</feed>"
    )


def day(n):
    return f"2026-09-{n:02d}T10:00:00Z"


RSS_ITEMS = [
    ("post-2", "<![CDATA[Second <b>post</b> &amp; more]]>", "https://blog.example/2?a=1&amp;b=2", "Tue, 02 Sep 2026 10:00:00 GMT"),
    ("post-1", "First post &#x41;&#66; &lt;3 AT&amp;T", "https://blog.example/1", "Mon, 01 Sep 2026 10:00:00 GMT"),
]
ATOM_ENTRIES = [("tag:atom.example,2026:2", "Atom two", "https://atom.example/2", day(2)),
                ("tag:atom.example,2026:1", "Atom one", "https://atom.example/1", day(1))]
VIDEOS = [("vid3", "Video 3", day(3)), ("vid2", "Video 2", day(2)), ("vid1", "Video 1", day(1))]
MALFORMED = "<html><body>Rate limited</body></html>"


def envelope(payload):
    return dict(tok.split("=", 1) for tok in payload.split("\n", 1)[0].split(" "))


def tick(state, routes, args=None, raising=False):
    http = FakeFeeds(routes, raising=raising)
    res = run(load("rss.py", http)["check"](state, args or {"urls": BLOG}))
    assert len(http.calls) <= 8, http.calls
    assert len(res["payload"].encode("utf-8")) <= 4096
    assert len(json.dumps(res["state"]).encode("utf-8")) <= 16384
    return res, http


def seeded(routes, args=None):
    res, _ = tick({}, routes, args)
    assert res["fire"] is False
    return res["state"]


class Seeding(unittest.TestCase):
    def test_empty_state_seeds_without_firing(self):
        res, http = tick({}, {BLOG: rss(RSS_ITEMS)})
        self.assertFalse(res["fire"])
        self.assertEqual(envelope(res["payload"]), {"subject": "feed/blog.example", "events": "seeded", "count": "2"})
        self.assertIn("seeded 2 entries from 1 feeds", res["payload"])
        self.assertEqual(res["state"]["seen"], ["post-2", "post-1"])
        self.assertTrue(res["state"]["seeded"])
        self.assertEqual(http.calls[0][3], {})  # no conditional headers on a bare first GET
        self.assertEqual(res["state"]["feeds"][BLOG]["title"], "Example & Blog")

    def test_backfill_fires_the_two_newest(self):
        res, _ = tick({}, {YT: youtube(VIDEOS)}, {"urls": YT, "backfill": "2"})
        self.assertTrue(res["fire"])
        self.assertEqual(envelope(res["payload"]), {"subject": "channel/Cool_Channel", "events": "new", "count": "2"})
        lines = res["payload"].split("\n")
        self.assertTrue(lines[1].startswith("id=vid3 "))
        self.assertTrue(lines[2].startswith("id=vid2 "))
        self.assertIn("seeded 1 entries from 1 feeds", res["payload"])
        self.assertEqual(res["state"]["seen"], ["vid3", "vid2", "vid1"])

    def test_newly_added_feed_seeds_while_the_other_fires(self):
        state = seeded({BLOG: rss(RSS_ITEMS)})
        new_items = [("post-3", "Third", "https://blog.example/3", "Wed, 03 Sep 2026 10:00:00 GMT")] + RSS_ITEMS
        res, _ = tick(state, {BLOG: rss(new_items), ATOM: atom(ATOM_ENTRIES)}, {"urls": f"{BLOG},{ATOM}"})
        self.assertTrue(res["fire"])
        self.assertEqual(envelope(res["payload"])["count"], "1")
        self.assertEqual(res["payload"].split("\n")[1].split(" ")[0], "id=post-3")
        self.assertIn("seeded 2 entries from 1 feeds", res["payload"])
        self.assertEqual(set(res["state"]["seen"]), {"post-3", "post-2", "post-1", "tag:atom.example,2026:2", "tag:atom.example,2026:1"})


class Firing(unittest.TestCase):
    def test_new_entry_fires_with_exact_envelope_and_line(self):
        state = seeded({BLOG: rss(RSS_ITEMS)})
        new_items = [("post-3", "Third &quot;quoted&quot;", "https://blog.example/3", "Wed, 03 Sep 2026 10:00:00 GMT")] + RSS_ITEMS
        res, _ = tick(state, {BLOG: rss(new_items)})
        self.assertTrue(res["fire"])
        lines = res["payload"].split("\n")
        self.assertEqual(lines[0], "subject=feed/blog.example events=new count=1")
        self.assertEqual(lines[1], 'id=post-3 published=2026-09-03T10:00:00 title=Third "quoted" url=https://blog.example/3')
        self.assertEqual(len(lines), 2)
        self.assertEqual(res["state"]["seen"], ["post-3", "post-2", "post-1"])

    def test_cdata_and_entities_decoded(self):
        res, _ = tick({}, {BLOG: rss(RSS_ITEMS)}, {"urls": BLOG, "backfill": "2"})
        lines = res["payload"].split("\n")
        self.assertEqual(lines[1], "id=post-2 published=2026-09-02T10:00:00 title=Second post & more url=https://blog.example/2?a=1&b=2")
        self.assertEqual(lines[2], "id=post-1 published=2026-09-01T10:00:00 title=First post AB &lt;3 AT&T url=https://blog.example/1")

    def test_two_feeds_one_304(self):
        state = seeded({BLOG: rss(RSS_ITEMS), YT: youtube(VIDEOS)}, {"urls": f"{BLOG},{YT}"})
        self.assertEqual(len(state["seen"]), 5)
        res, http = tick(state, {BLOG: (304, ""), YT: youtube([("vid4", "Video 4", day(4))] + VIDEOS)}, {"urls": f"{BLOG},{YT}"})
        self.assertEqual(len(http.calls), 2)
        self.assertTrue(res["fire"])
        self.assertEqual(envelope(res["payload"]), {"subject": "feeds/2", "events": "new", "count": "1"})
        self.assertIn("id=vid4 published=2026-09-04T10:00:00Z title=Video 4 url=https://www.youtube.com/watch?v=vid4", res["payload"])
        self.assertEqual(res["state"]["feeds"][BLOG], state["feeds"][BLOG])
        self.assertEqual(res["state"]["seen"][0], "vid4")

    def test_filter_excludes_titles(self):
        state = seeded({YT: youtube(VIDEOS)}, {"urls": YT, "filter": "Video"})
        new = [("live1", "LIVE stream", day(5)), ("vid4", "Video 4", day(4))] + VIDEOS
        res, _ = tick(state, {YT: youtube(new)}, {"urls": YT, "filter": "^Video"})
        self.assertTrue(res["fire"])
        self.assertEqual(envelope(res["payload"])["count"], "1")
        self.assertNotIn("live1", res["payload"])
        self.assertNotIn("live1", res["state"]["seen"])

    def test_newest_first_across_feeds(self):
        state = seeded({BLOG: rss(RSS_ITEMS), ATOM: atom(ATOM_ENTRIES)}, {"urls": f"{BLOG},{ATOM}"})
        routes = {
            BLOG: rss([("post-3", "Third", "https://blog.example/3", "Wed, 03 Sep 2026 10:00:00 GMT")] + RSS_ITEMS),
            ATOM: atom([("tag:atom.example,2026:9", "Atom nine", "https://atom.example/9", day(9))] + ATOM_ENTRIES),
        }
        res, _ = tick(state, routes, {"urls": f"{BLOG},{ATOM}"})
        ids = [ln.split(" ")[0] for ln in res["payload"].split("\n")[1:]]
        self.assertEqual(ids, ["id=tag:atom.example,2026:9", "id=post-3"])


class Conditional(unittest.TestCase):
    HEADERS = {"etag": 'W/"abc"', "last-modified": "Tue, 02 Sep 2026 10:00:00 GMT", "cache-control": "max-age=900"}

    def test_etag_and_last_modified_sent_back(self):
        state = seeded({BLOG: (200, rss(RSS_ITEMS), self.HEADERS)})
        self.assertEqual(state["feeds"][BLOG]["etag"], 'W/"abc"')
        res, http = tick(state, {BLOG: (304, "")})
        self.assertEqual(http.calls[0][3], {"If-None-Match": 'W/"abc"', "If-Modified-Since": "Tue, 02 Sep 2026 10:00:00 GMT"})
        self.assertFalse(res["fire"])
        self.assertEqual(res["payload"], "subject=feed/blog.example events= count=0")
        self.assertEqual(res["state"]["seen"], state["seen"])
        self.assertEqual(res["state"]["feeds"], state["feeds"])

    def test_304_raised_as_the_monty_bridge_does(self):
        state = seeded({BLOG: (200, rss(RSS_ITEMS), self.HEADERS)})
        res, _ = tick(state, {BLOG: (304, "")}, raising=True)
        self.assertFalse(res["fire"])
        self.assertEqual(res["state"], state)
        self.assertEqual(res["payload"], "subject=feed/blog.example events= count=0")

        async def plain_304(url, method, body=None, headers=None):
            raise RuntimeError(f"http_request: HTTP 304 for GET {url} (0 bytes, 1 ms)")

        res = run(load("rss.py", plain_304)["check"](state, {"urls": BLOG}))
        self.assertEqual(res["state"], state)

    def test_missing_location_without_conditionals_is_an_error(self):
        async def no_location(url, method, body=None, headers=None):
            raise RuntimeError("http_request: redirect missing Location header")

        out = run(load("rss.py", no_location)["probe"]({"urls": BLOG}))
        self.assertEqual(out, "blog.example: http_request: redirect missing Location header")

    def test_next_from_max_age_and_sy_update_period(self):
        res, _ = tick({}, {BLOG: (200, rss(RSS_ITEMS), self.HEADERS)})
        self.assertEqual(res["next"], 900)
        body = rss(RSS_ITEMS).replace("<channel>", "<channel><sy:updatePeriod>hourly</sy:updatePeriod><sy:updateFrequency>2</sy:updateFrequency>")
        res, _ = tick({}, {BLOG: body})
        self.assertEqual(res["next"], 1800)
        res, _ = tick({}, {BLOG: rss(RSS_ITEMS)})
        self.assertIsNone(res["next"])

    def test_redirect_target_is_reused(self):
        final = "https://cdn.example/feed.xml"
        http = FakeFeeds({BLOG: (200, rss(RSS_ITEMS), self.HEADERS)})

        async def redirecting(url, method, body=None, headers=None):
            reply = await http(url, method, body, headers)
            reply["url"] = final
            return reply

        res = run(load("rss.py", redirecting)["check"]({}, {"urls": BLOG}))
        self.assertEqual(res["state"]["feeds"][BLOG]["url"], final)
        res2, http2 = tick(res["state"], {final: (304, "")})
        self.assertEqual(http2.calls[0][1], final)
        self.assertFalse(res2["fire"])


class Errors(unittest.TestCase):
    def test_malformed_feed_is_a_diagnostic_not_an_exception(self):
        res, _ = tick({}, {BLOG: MALFORMED})
        self.assertFalse(res["fire"])
        self.assertEqual(envelope(res["payload"]), {"subject": "feed/blog.example", "events": "", "count": "0"})
        self.assertIn(f"feed=blog.example error=no entries parsed ({len(MALFORMED)} bytes)", res["payload"])
        self.assertEqual(res["state"]["seen"], [])

    def test_one_feed_erroring_does_not_break_the_check(self):
        state = seeded({BLOG: rss(RSS_ITEMS), BAD: atom(ATOM_ENTRIES)}, {"urls": f"{BLOG},{BAD}"})
        new_items = [("post-3", "Third", "https://blog.example/3", "Wed, 03 Sep 2026 10:00:00 GMT")] + RSS_ITEMS
        for raising in (False, True):
            res, _ = tick(state, {BLOG: rss(new_items), BAD: (500, "oops")}, {"urls": f"{BLOG},{BAD}"}, raising=raising)
            self.assertTrue(res["fire"])
            self.assertEqual(envelope(res["payload"])["count"], "1")
            self.assertIn("feed=bad.example error=HTTP 500", res["payload"])
            self.assertEqual(res["state"]["feeds"][BAD], {"title": "Atom Feed", "kind": "atom"})
            self.assertIn("tag:atom.example,2026:1", res["state"]["seen"])

    def test_every_feed_failing_raises(self):
        for raising in (False, True):
            with self.assertRaises(RuntimeError):
                tick({}, {BLOG: (500, "oops"), BAD: (503, "down")}, {"urls": f"{BLOG},{BAD}"}, raising=raising)

    def test_eight_urls_refused(self):
        urls = ",".join(f"https://f{i}.example/feed" for i in range(8))
        ns = load("rss.py", FakeFeeds({}))
        with self.assertRaises(ValueError) as cm:
            run(ns["check"]({}, {"urls": urls}))
        self.assertEqual(str(cm.exception), "urls: at most 7")
        self.assertEqual(run(ns["probe"]({"urls": urls})), "urls: at most 7")
        with self.assertRaises(ValueError):
            run(ns["check"]({}, {}))
        self.assertEqual(run(ns["probe"]({"urls": BLOG, "filter": "("})), "filter= is not a valid regex")
        self.assertEqual(run(ns["probe"]({"urls": "ftp://x"})), "urls: ftp://x is not an http(s) URL")


class Budgets(unittest.TestCase):
    def test_seen_fifo_bound_holds(self):
        items = [(f"id-{i}", f"Post {i}", f"https://blog.example/{i}", f"Tue, 02 Sep 2026 {i // 60:02d}:{i % 60:02d}:00 GMT") for i in range(350)]
        res, _ = tick({}, {BLOG: rss(items)})
        self.assertEqual(len(res["state"]["seen"]), 300)
        self.assertEqual(res["state"]["seen"][0], "id-349")
        self.assertNotIn("id-0", res["state"]["seen"])

    def test_state_under_16k_with_long_ids(self):
        items = [(f"https://blog.example/{i}/" + "x" * 150, f"Post {i}", f"https://blog.example/{i}", "") for i in range(350)]
        res, _ = tick({}, {BLOG: rss(items)})
        self.assertLessEqual(len(json.dumps(res["state"]).encode("utf-8")), 16384)
        self.assertTrue(all(len(s) == 120 for s in res["state"]["seen"]))
        self.assertGreater(len(res["state"]["seen"]), 50)

    def test_payload_truncated_at_4k_with_200_new(self):
        state = seeded({BLOG: rss(RSS_ITEMS)})
        items = [(f"new-{i}", f"Brand new post number {i} \u2603", f"https://blog.example/new/{i}", day(5)) for i in range(200)] + RSS_ITEMS
        res, _ = tick(state, {BLOG: rss(items)})
        self.assertTrue(res["fire"])
        self.assertEqual(envelope(res["payload"])["count"], "200")
        payload = res["payload"].encode("utf-8")
        self.assertLessEqual(len(payload), 4096)
        self.assertGreater(len(payload), 3400)
        more = re.search(r"^\+(\d+) more$", res["payload"], re.M)
        self.assertIsNotNone(more)
        shown = len([ln for ln in res["payload"].split("\n") if ln.startswith("id=")])
        self.assertEqual(shown + int(more.group(1)), 200)
        self.assertEqual(len(res["state"]["seen"]), 202)


class Probe(unittest.TestCase):
    def test_per_feed_counts(self):
        routes = {BLOG: rss(RSS_ITEMS), YT: youtube(VIDEOS), ATOM: MALFORMED}
        http = FakeFeeds(routes)
        out = run(load("rss.py", http)["probe"]({"urls": f"{BLOG},{YT},{ATOM},{BAD}"}))
        self.assertEqual(
            out,
            "blog.example: 2 entries, parsed as rss; www.youtube.com: 3 entries, parsed as youtube; "
            f"atom.example: no entries parsed ({len(MALFORMED)} bytes); bad.example: HTTP 404",
        )
        self.assertTrue(all(h == {} for _, _, _, h in http.calls))

    def test_never_raises_under_the_bridge(self):
        out = run(load("rss.py", FakeFeeds({}, raising=True))["probe"]({"urls": BAD}))
        self.assertEqual(out, "bad.example: HTTP 404")


class Lint(unittest.TestCase):
    def test_monty_subset_and_header(self):
        src = (SENSORS / "rss.py").read_text(encoding="utf-8")
        for pat in (r"^\s*import (os|xml|html|urllib|json)\b", r"^\s*from \w", r"\byield\b", r"^\s*class ", r"\bopen\(",
                    r"\bsubprocess\b", "%", r":=", r"\{\{secret:\w", r"\bsetdefault\b"):
            self.assertIsNone(re.search(pat, src, re.M), pat)
        header = src.split("\n")[:16]
        self.assertEqual(header[0], "# theuth-sensor: rss 1")
        args = [h for h in header if h.startswith("# arg: ")]
        self.assertEqual([a.split()[2:4] for a in args], [["urls", "required"], ["filter", "optional"], ["backfill", "optional"]])
        self.assertTrue(all(" — " in a for a in args))
        self.assertFalse(any(h.startswith("# needs:") or h.startswith("# hosts:") for h in header))
        self.assertTrue(any(h.startswith("when: rss.py urls=") for h in header))


if __name__ == "__main__":
    unittest.main()
