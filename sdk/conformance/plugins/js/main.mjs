// Conformance fixture (not a template): every SDK implements this same behavior; see ../../host.py.
import { RpcError, plugin } from "./theuth-plugin.js";

function paint() {
  const n = plugin.state.int("n");
  plugin.status({ dot: "ok", badge: n, label: String(n) });
  plugin.ui({ text: `${n} presses`, buttons: [["Press", "inc"]] });
}

plugin.on_start(async () => {
  paint();
  plugin.log("pong", { fields: { now_ms: (await plugin.ping()).now_ms } });
});
plugin.on_render(() => paint()); // without it, render re-runs on_start (which pings here)

plugin.action("inc", () => {
  plugin.state.add("n", 1);
  paint();
});
plugin.action("deadlock", () => plugin.request("ping", {}));
plugin.action("call", () => plugin.call("echo", { x: 1 }));
plugin.action("denied", async () => {
  try {
    await plugin.call("shell", {});
  } catch (e) {
    if (e instanceof RpcError) return { code: e.code };
    throw e;
  }
});
plugin.action("boom", () => {
  throw new Error("boom");
});
plugin.action("extras", (req) => {
  plugin.emit("pressed", { n: plugin.state.int("n") });
  plugin.notify("Hi", "there");
  plugin.invalidate();
  plugin.send("custom/note", { a: 1 });
  return req.args;
});

plugin.tool("presses", () => String(plugin.state.int("n")));
plugin.tool("fail", () => {
  throw new Error("kaput");
});
plugin.command("count", () => `n=${plugin.state.int("n")}`);

plugin.on("conformance.tick", (ev) => plugin.log("tick", { fields: { k: ev.data.data.k } }));
plugin.on("agent.events#TurnFinished", (ev) => plugin.log("turn", { fields: { stop_reason: ev.data.stop_reason } }));

plugin.on_request("custom/echo", (params) => params);
plugin.on_granted((grants) => plugin.log("granted", { fields: { placements: grants.placements.length } }));
plugin.on_shutdown(() => plugin.log("bye"));
await plugin.run();
