/** 123 W2: `Funnel` parity (see `chart-parity.tsx`); unit ids are stage indices as strings. */
import { parity } from "./chart-parity.tsx";

parity({
  name: "Funnel",
  prefix: "funnel",
  app: `root = Stack([f, out])
$lit = ["0"]
$selected = ""
$value = ""
f = Funnel([{label: "Visited", value: 1200}, {label: "Signed up", value: 300}, {label: "Paid", value: 45}], "funnel", null, "Signup", $lit, Action([@Set($selected, $value)]))
out = TextContent("Selected: " + $selected)`,
  units: (p) => ((p.stages as unknown[]) ?? []).map((_, i) => String(i)),
  press: "1",
  lit: ["0"],
});
