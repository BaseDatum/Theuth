#!/usr/bin/env python3
"""theuth CPython sensor shim (design 40 §4.4). A dotfile: not a sensor.

    python3 .shim.py <sensors>/<sensor.py>
      stdin : {"fn": "check" | "probe", "state": {...}, "args": {...}}
      stdout: the result JSON on the last line; earlier lines are the sensor's
              prints (kept as the strike tail when the child fails)
      env   : THEUTH_SENSOR_<KEY> per arg, THEUTH_SENSOR_CWD, THEUTH_SENSOR_TIMEOUT,
              THEUTH_SENSOR_HOSTS (comma list: hosts http_request may POST to),
              THEUTH_SENSOR_PROBE=1 for probe
      cwd   : the schedule file's cwd

The sensor is exec'd with the same read-only tool names Monty binds
(`http_request`, `read_file`, `glob`, `list_dir`; `knowledge_search` is not
available here), implemented on urllib/os under the same rules: GET anywhere,
POST only to a declared host, paths rooted at cwd. No secret reaches this
process: the harness refuses a cpython sensor whose args mention a
`{{secret:...}}` placeholder. Plain Python 3.8+, stdlib only.
"""
import asyncio
import json
import os
import sys
import time
import urllib.error
import urllib.request
from urllib.parse import urlsplit

CWD = os.path.realpath(os.environ.get("THEUTH_SENSOR_CWD") or os.getcwd())
HOSTS = [h.strip().lower() for h in os.environ.get("THEUTH_SENSOR_HOSTS", "").split(",") if h.strip()]
MAX_BYTES = 1 << 20
RESERVED = ("THEUTH_SENSOR_CWD", "THEUTH_SENSOR_TIMEOUT", "THEUTH_SENSOR_HOSTS", "THEUTH_SENSOR_PROBE")


def _rooted(name, path):
    p = CWD if path is None else str(path)
    if p == "/cwd" or p.startswith("/cwd/"):
        p = p[5:] or "."
    real = os.path.realpath(os.path.join(CWD, p))
    if real != CWD and not real.startswith(CWD + os.sep):
        raise PermissionError("%s: %s is outside cwd" % (name, path))
    return real


async def http_request(url, method="GET", body=None, headers=None):
    method = (method or "GET").strip().upper()
    parts = urlsplit(url)
    host = (parts.hostname or "").lower()
    if method == "POST":
        if host not in HOSTS:
            raise PermissionError("http_request: POST to %s is not allowed; declare it in `# hosts:`" % host)
    elif method != "GET":
        raise PermissionError("http_request: method %s is not allowed in a sensor (GET, or POST to `# hosts:`)" % method)
    if parts.scheme not in ("http", "https"):
        raise ValueError("http_request: url must be http(s)")
    hdrs = dict(headers or {})
    data = None
    if body is not None:
        if isinstance(body, str):
            data = body.encode("utf-8")
        else:
            data = json.dumps(body).encode("utf-8")
            if not any(k.lower() == "content-type" for k in hdrs):
                hdrs["Content-Type"] = "application/json"

    def go():
        started = time.monotonic()
        req = urllib.request.Request(url, data=data, headers=hdrs, method=method)
        try:
            with urllib.request.urlopen(req, timeout=30) as r:
                raw, status, rh, final = r.read(MAX_BYTES + 1), r.status, r.headers, r.geturl()
        except urllib.error.HTTPError as e:
            raw, status, rh, final = e.read(MAX_BYTES + 1), e.code, e.headers, url
        truncated = len(raw) > MAX_BYTES
        raw = raw[:MAX_BYTES]
        if status < 200 or status >= 300 or truncated:
            raise RuntimeError("http_request: HTTP %d for %s %s%s" % (status, method, url, " (body cut at 1 MiB)" if truncated else ""))
        rh = {k.lower(): v for k, v in rh.items()}
        text = raw.decode("utf-8", "replace")
        ct = rh.get("content-type", "").split(";")[0].strip().lower()
        parsed = None
        if raw and (ct in ("application/json", "text/json") or ct.endswith("+json")):
            try:
                parsed = json.loads(text)
            except ValueError:
                raise RuntimeError("http_request: reply declared %s but is not valid JSON" % ct)
        return {
            "url": final,
            "status": status,
            "headers": rh,
            "body": parsed if parsed is not None else (text if raw else None),
            "truncated": False,
            "took_ms": int((time.monotonic() - started) * 1000),
        }

    return await asyncio.get_running_loop().run_in_executor(None, go)


async def read_file(path, **_ignored):
    # ponytail: raw text only; the harness's read_file pagination/numbering is not mirrored.
    with open(_rooted("read_file", path), encoding="utf-8", errors="replace") as f:
        return f.read()


async def glob(pattern, path=None, limit=500, hidden=False):
    import fnmatch

    root = _rooted("glob", path)
    by_name = "/" not in pattern
    found = []
    truncated = False
    for dirpath, dirnames, filenames in os.walk(root):
        if not hidden:
            dirnames[:] = [d for d in dirnames if not d.startswith(".")]
            filenames = [f for f in filenames if not f.startswith(".")]
        for f in filenames:
            rel = os.path.relpath(os.path.join(dirpath, f), root).replace(os.sep, "/")
            if fnmatch.fnmatch(f if by_name else rel, pattern):
                if len(found) >= limit:
                    truncated = True
                    break
                found.append(rel)
        if truncated:
            break
    return {"files": found, "truncated": truncated}


async def list_dir(path):
    root = _rooted("list_dir", path)
    names = []
    for e in os.listdir(root):
        names.append(e + "/" if os.path.isdir(os.path.join(root, e)) else e)
    return "\n".join(sorted(names))


def main():
    sensor = sys.argv[1]
    req = json.loads(sys.stdin.read() or "{}")
    fn = req.get("fn") or ("probe" if os.environ.get("THEUTH_SENSOR_PROBE") else "check")
    args = dict(req.get("args") or {})
    for k, v in os.environ.items():
        if k.startswith("THEUTH_SENSOR_") and k not in RESERVED:
            args.setdefault(k[len("THEUTH_SENSOR_"):].lower(), v)
    args.setdefault("cwd", CWD)
    ns = {
        "__name__": "__sensor__",
        "__file__": sensor,
        "http_request": http_request,
        "read_file": read_file,
        "glob": glob,
        "list_dir": list_dir,
    }
    with open(sensor, encoding="utf-8") as f:
        code = f.read()
    exec(compile(code, sensor, "exec"), ns)
    target = ns.get(fn)
    if fn == "probe":
        result = asyncio.run(target(args)) if target is not None else None
    elif target is None:
        raise RuntimeError("%s defines no `async def check(state, args)`" % sensor)
    else:
        result = asyncio.run(target(req.get("state") or {}, args))
    sys.stdout.write("\n" + json.dumps(result) + "\n")
    sys.stdout.flush()


if __name__ == "__main__":
    main()
