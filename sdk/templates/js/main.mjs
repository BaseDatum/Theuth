import { plugin } from "./theuth-plugin.js";
function paint() {
  const n = plugin.state.int("n");
  plugin.status({ dot: "ok", badge: n, label: String(n) });
  plugin.ui({ text: `${n} presses`, buttons: [["Press", "inc"]] });
}
plugin.action("inc", () => { plugin.state.add("n", 1); paint(); });
plugin.tool("presses", () => String(plugin.state.int("n")));
plugin.on_start(paint);
await plugin.run();
