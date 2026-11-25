from theuth_plugin import plugin
def paint():
    n = plugin.state.int("n")
    plugin.status(dot="ok", badge=n, label=str(n))
    plugin.ui(text=f"{n} presses", buttons=[("Press", "inc")])
@plugin.action("inc")
def inc(_):
    plugin.state.add("n", 1); paint()
@plugin.tool("presses")
def presses(_):
    return str(plugin.state.int("n"))
plugin.on_start(paint)
plugin.run()
