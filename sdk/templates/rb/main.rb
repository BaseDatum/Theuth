require_relative "theuth_plugin"
def paint
  n = plugin.state.int("n")
  plugin.status(dot: "ok", badge: n, label: n.to_s)
  plugin.ui(text: "#{n} presses", buttons: [["Press", "inc"]])
end
plugin.action("inc") { plugin.state.add("n", 1); paint }
plugin.tool("presses") { plugin.state.int("n").to_s }
plugin.on_start { paint }
plugin.run
