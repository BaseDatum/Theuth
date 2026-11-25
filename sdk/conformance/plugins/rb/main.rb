# Conformance fixture (not a template): every SDK implements this same behavior; see ../../host.py.
require_relative "theuth_plugin"

def paint
  n = plugin.state.int("n")
  plugin.status(dot: "ok", badge: n, label: n.to_s)
  plugin.ui(text: "#{n} presses", buttons: [["Press", "inc"]])
end

plugin.on_start do
  paint
  plugin.log("pong", fields: { "now_ms" => plugin.ping["now_ms"] })
end
plugin.on_render { paint } # without it, render re-runs on_start (which pings here)

plugin.action("inc") { plugin.state.add("n", 1); paint; nil }
plugin.action("deadlock") { plugin.request("ping", {}) }
plugin.action("call") { plugin.call("echo", { "x" => 1 }) }
plugin.action("denied") do
  plugin.call("shell", {})
rescue Theuth::RpcError => e
  { "code" => e.code }
end
plugin.action("boom") { raise "boom" }
plugin.action("extras") do |req|
  plugin.emit("pressed", { "n" => plugin.state.int("n") })
  plugin.notify("Hi", "there")
  plugin.invalidate
  plugin.send("custom/note", { "a" => 1 })
  req.args
end

plugin.tool("presses") { plugin.state.int("n").to_s }
plugin.tool("fail") { raise "kaput" }
plugin.command("count") { "n=#{plugin.state.int('n')}" }

plugin.on("conformance.tick") { |ev| plugin.log("tick", fields: { "k" => ev.data["data"]["k"] }) }
plugin.on("agent.events#TurnFinished") { |ev| plugin.log("turn", fields: { "stop_reason" => ev.data["stop_reason"] }) }

plugin.on_request("custom/echo") { |params| params }
plugin.on_granted { |grants| plugin.log("granted", fields: { "placements" => grants["placements"].size }) }
plugin.on_shutdown { plugin.log("bye") }
plugin.run
