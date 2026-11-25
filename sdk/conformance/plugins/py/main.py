# Conformance fixture (not a template): every SDK implements this same behavior; see ../../host.py.
from theuth_plugin import RpcError, plugin


def paint():
    n = plugin.state.int("n")
    plugin.status(dot="ok", badge=n, label=str(n))
    plugin.ui(text=f"{n} presses", buttons=[("Press", "inc")])


@plugin.on_start
def start():
    paint()
    plugin.log("pong", fields={"now_ms": plugin.ping()["now_ms"]})


plugin.on_render(lambda req: paint())  # without it, render re-runs on_start (which pings here)


@plugin.action("inc")
def inc(_):
    plugin.state.add("n", 1)
    paint()


@plugin.action("deadlock")
def deadlock(_):
    return plugin.request("ping", {})


@plugin.action("call")
def call(_):
    return plugin.call("echo", {"x": 1})


@plugin.action("denied")
def denied(_):
    try:
        plugin.call("shell", {})
    except RpcError as e:
        return {"code": e.code}


@plugin.action("boom")
def boom(_):
    raise ValueError("boom")


@plugin.action("extras")
def extras(req):
    plugin.emit("pressed", {"n": plugin.state.int("n")})
    plugin.notify("Hi", "there")
    plugin.invalidate()
    plugin.send("custom/note", {"a": 1})
    return req.args


@plugin.tool("presses")
def presses(_):
    return str(plugin.state.int("n"))


@plugin.tool("fail")
def fail(_):
    raise RuntimeError("kaput")


@plugin.command("count")
def count(_):
    return f"n={plugin.state.int('n')}"


@plugin.on("conformance.tick")
def tick(ev):
    plugin.log("tick", fields={"k": ev.data["data"]["k"]})


@plugin.on("agent.events#TurnFinished")
def turn(ev):
    plugin.log("turn", fields={"stop_reason": ev.data["stop_reason"]})


plugin.on_request("custom/echo", lambda params: params)
plugin.on_granted(lambda grants: plugin.log("granted", fields={"placements": len(grants["placements"])}))
plugin.on_shutdown(lambda: plugin.log("bye"))
plugin.run()
