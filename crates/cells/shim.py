# Owner: E2 — the python shim of design 171 §5.8 (stdlib only, Python >= 3.8, <= 250 lines).
# Binary-owned: written to <data dir>/cells/shim.py at engine start; edits there are overwritten.
# One JSON object per line. Engine -> shim on stdin: execute, input_reply, interrupt. Shim ->
# engine on a PRIVATE dup of fd 1: stream, display, result, error, input_request, status. fd 1
# and 2 are re-pointed at pipes forwarded as `stream` frames (50 ms or 4 KiB), so output of C
# extensions and subprocesses is captured too.
import ast, base64, builtins, codecs, getpass, io, json, linecache, os, queue, sys, threading, time, traceback
import _thread

os.environ.setdefault("MPLBACKEND", "Agg")
FRAMES = io.open(os.dup(1), "w", encoding="utf-8", newline="\n")
CTRL = io.open(os.dup(0), "r", encoding="utf-8", newline="\n")
os.dup2(os.open(os.devnull, os.O_RDONLY), 0)  # a subprocess reading stdin must never eat frames
LOCK = threading.Lock()
MARK = b"\x00\x1b[theuth-shim-sync]\x00"
STATE = {"id": None, "count": 0}
INBOX = queue.Queue()


def send(**frame):
    line = json.dumps(frame, default=str, ensure_ascii=False)
    with LOCK:
        FRAMES.write(line + "\n")
        FRAMES.flush()


class Pump(object):
    """One re-pointed fd: a reader thread buffers, `flush` emits `stream` frames."""

    def __init__(self, name, fd):
        self.name, self.fd, self.buf = name, fd, b""
        self.lock = threading.Lock()
        self.synced = threading.Event()
        self.decoder = codecs.getincrementaldecoder("utf-8")("replace")
        rd, wr = os.pipe()
        os.dup2(wr, fd)
        os.close(wr)
        self.rd = rd
        threading.Thread(target=self.read, daemon=True).start()

    def read(self):
        while True:
            try:
                data = os.read(self.rd, 65536)
            except OSError:
                return
            if not data:
                return
            with self.lock:
                self.buf += data
                now = len(self.buf) >= 4096 or MARK in self.buf
            if now:
                self.flush()

    def flush(self):
        with self.lock:
            at = self.buf.find(MARK)
            if at >= 0:
                out, self.buf = self.buf[:at], self.buf[at + len(MARK):]
            else:  # keep a possible partial marker at the tail for the next read
                keep = next((n for n in range(min(len(MARK) - 1, len(self.buf)), 0, -1)
                             if self.buf.endswith(MARK[:n])), 0)
                cut = len(self.buf) - keep
                out, self.buf = self.buf[:cut], self.buf[cut:]
            text = self.decoder.decode(out, final=at >= 0)
        if text:
            send(type="stream", id=STATE["id"], name=self.name, text=text)
        if at >= 0:
            self.synced.set()

    def sync(self):
        """Every byte written to the fd before now has been sent."""
        self.synced.clear()
        os.write(self.fd, MARK)
        self.synced.wait(2.0)


PUMPS = []


def drain():
    """Send every byte printed so far before the next structured frame (output order)."""
    try:
        sys.stdout.flush()
        sys.stderr.flush()
    except Exception:
        pass
    for p in PUMPS:
        p.sync()


def ticker():
    while True:
        time.sleep(0.05)
        for p in PUMPS:
            if p.buf:
                p.flush()


def reader():
    for line in CTRL:
        try:
            frame = json.loads(line)
        except ValueError:
            continue
        if frame.get("type") == "interrupt":
            _thread.interrupt_main()
        else:
            INBOX.put(frame)
    os._exit(0)  # the engine is gone (stdin closed): never outlive it, even mid-run


def next_frame():
    while True:
        try:
            return INBOX.get(timeout=0.1)
        except queue.Empty:
            continue


def ask(prompt="", password=False):
    drain()
    send(type="input_request", id=STATE["id"], prompt=str(prompt), password=bool(password))
    while True:
        frame = next_frame()
        if frame.get("type") == "input_reply":
            return frame.get("value", "")


builtins.input = lambda prompt="": ask(prompt, False)
getpass.getpass = lambda prompt="Password: ", stream=None: ask(prompt, True)
REPRS = (("_repr_html_", "text/html"), ("_repr_markdown_", "text/markdown"),
         ("_repr_svg_", "image/svg+xml"), ("_repr_png_", "image/png"),
         ("_repr_json_", "application/json"))


def bundle(value):
    data = {}
    if not isinstance(value, type):
        for attr, mime in REPRS:
            fn = getattr(value, attr, None)
            if not callable(fn):
                continue
            try:
                out = fn()
            except Exception:
                continue
            if isinstance(out, tuple):
                out = out[0]
            if out is None:
                continue
            if isinstance(out, bytes):
                out = base64.b64encode(out).decode("ascii")
            data[mime] = out
    try:
        data["text/plain"] = repr(value)
    except Exception as e:
        data["text/plain"] = "<unrepresentable %s: %s>" % (type(value).__name__, e)
    return data


def display(*values, **kw):
    drain()
    for v in values:
        send(type="display", id=STATE["id"], data=bundle(v), metadata=kw.get("metadata") or {})


builtins.display = display


def figures():
    plt = sys.modules.get("matplotlib.pyplot")
    if plt is None:
        return
    for num in plt.get_fignums():
        fig = plt.figure(num)
        buf = io.BytesIO()
        fig.savefig(buf, format="png", bbox_inches="tight")
        png = base64.b64encode(buf.getvalue()).decode("ascii")
        send(type="display", id=STATE["id"], data={"image/png": png, "text/plain": repr(fig)}, metadata={})
    plt.close("all")


def error(exc):
    tb = [f for f in traceback.extract_tb(exc.__traceback__) if f.filename.startswith("<cell")]
    lines = ["Traceback (most recent call last):\n"] + traceback.format_list(tb) if tb else []
    lines += traceback.format_exception_only(type(exc), exc)
    send(type="error", id=STATE["id"], ename=type(exc).__name__, evalue=str(exc), traceback=lines)


NS = {"__name__": "__main__", "__builtins__": builtins}


def execute(frame):
    STATE["id"] = frame.get("id")
    send(type="status", state="busy", id=STATE["id"])
    ok = True
    try:
        if frame.get("cwd"):
            os.chdir(frame["cwd"])
        if frame.get("env"):
            os.environ.update(frame["env"])
        code = frame.get("code", "")
        STATE["count"] += 1
        name = "<cell %d>" % STATE["count"]
        linecache.cache[name] = (len(code), None, code.splitlines(True), name)
        tree = ast.parse(code, name, "exec")
        last = None
        # Every mode shows its last bare expression: a cell/selection like Jupyter, a line
        # like a REPL (171F4 T2-6); a statement prints nothing.
        if tree.body and isinstance(tree.body[-1], ast.Expr):
            last = ast.Expression(tree.body.pop().value)
        exec(compile(tree, name, "exec"), NS)
        if last is not None:
            value = eval(compile(last, name, "eval"), NS)
            if value is not None:
                NS["_"] = value
                drain()
                send(type="result", id=STATE["id"], data=bundle(value), count=STATE["count"])
        figures()
    except BaseException as e:  # KeyboardInterrupt and SystemExit end the run, not the kernel
        ok = False
        try:
            drain()
            error(e)
        except BaseException:
            send(type="error", id=STATE["id"], ename=type(e).__name__, evalue=str(e), traceback=[])
    finally:
        drain()
        send(type="status", state="idle", id=STATE["id"], ok=ok, cwd=os.getcwd())


def main():
    PUMPS.extend([Pump("stdout", 1), Pump("stderr", 2)])
    threading.Thread(target=ticker, daemon=True).start()
    send(type="status", state="ready", cwd=os.getcwd())
    threading.Thread(target=reader, daemon=True).start()
    while True:
        try:
            frame = next_frame()
        except KeyboardInterrupt:
            continue  # a Stop that raced the end of a run
        if frame.get("type") == "execute":
            try:
                execute(frame)
            except KeyboardInterrupt:  # a Stop landing in the run's own bookkeeping
                send(type="status", state="idle", id=STATE["id"], ok=False, cwd=os.getcwd())


if __name__ == "__main__":
    main()
