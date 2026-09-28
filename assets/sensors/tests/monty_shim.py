"""Run a theuth sensor file under CPython with a scripted `http_request`.

The sensor is `exec`'d unchanged with the tool injected as a global, so the
same file that runs in Monty runs here. Routes are keyed by (METHOD, path);
the path is matched after stripping the API origin and any query string.
"""
import asyncio
import pathlib

SENSORS = pathlib.Path(__file__).resolve().parent.parent
ORIGIN = "https://api.github.com"


class FakeHttp:
    """Scripted responder. `routes[(method, path)] = body` or `(status, body)`."""

    def __init__(self, routes):
        self.routes = dict(routes)
        self.calls = []

    async def __call__(self, url, method, body=None, headers=None):
        self.calls.append((method, url, body, headers))
        path = url[len(ORIGIN):] if url.startswith(ORIGIN) else url
        path = path.split("?", 1)[0]
        hit = self.routes.get((method, path))
        status, reply = 200, hit
        if isinstance(hit, tuple):
            status, reply = hit
        if hit is None:
            status, reply = 404, {"message": f"no route for {method} {path}"}
        return {"url": url, "status": status, "headers": {}, "body": reply, "truncated": False, "took_ms": 1}


def load(name, http):
    """exec the sensor `name` (e.g. "pr_watch.py") with `http` bound as `http_request`."""
    path = SENSORS / name
    ns = {"__name__": "sensor", "http_request": http}
    exec(compile(path.read_text(encoding="utf-8"), str(path), "exec"), ns)
    return ns


def run(coro):
    return asyncio.run(coro)
