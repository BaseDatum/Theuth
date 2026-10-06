# theuth-sensor: rss 1
# arg: urls     required url[,url] — feed URLs, comma-separated (Atom, RSS 2.0, YouTube videos.xml); up to 7
# arg: filter   optional -         — regex; only entries whose title matches count
# arg: backfill optional 0         — fire on the N newest entries at the first check instead of seeding silently
"""New entries in one or more feeds (design 108 §10). Placeholder: the RSS worker's file replaces this body.

when: rss.py urls=<url>[,<url>…] [filter=<regex on title>] [backfill=0]
"""


async def check(state: dict, args: dict) -> dict:
    return {"fire": False, "payload": "subject=feed events=seeded count=0", "state": state, "next": None}
