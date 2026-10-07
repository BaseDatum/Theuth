/** 123 W2: `Waterfall` parity (see `chart-parity.tsx`); unit ids are step indices as strings. */
import { parity } from "./chart-parity.tsx";

parity({
  name: "Waterfall",
  prefix: "waterfall",
  app: `root = Stack([w, out])
$lit = ["1"]
$selected = ""
$value = ""
w = Waterfall([{label: "Jan", value: 100, kind: "total"}, {label: "Hires", value: 12}, {label: "Leavers", value: -5, state: "danger"}, {label: "Dec", value: 107, kind: "total"}], "people", "Headcount", $lit, Action([@Set($selected, $value)]))
out = TextContent("Selected: " + $selected)`,
  units: (p) => ((p.steps as unknown[]) ?? []).map((_, i) => String(i)),
  press: "2",
  lit: ["1"],
});
