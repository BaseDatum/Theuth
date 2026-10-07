/** 123 W2: `Gantt` parity (see `chart-parity.tsx`); unit ids are task ids. */
import { parity } from "./chart-parity.tsx";

parity({
  name: "Gantt",
  prefix: "gantt",
  app: `root = Stack([g, out])
$lit = ["a"]
$selected = ""
$value = ""
g = Gantt([{id: "a", label: "Kernel", start: "2026-10-08", end: "2026-10-10", progress: 1}, {id: "b", label: "Charts", start: "2026-10-10", end: "2026-10-14", depends: ["a"]}], "2026-10-11", "Wave", $lit, Action([@Set($selected, $value)]))
out = TextContent("Selected: " + $selected)`,
  units: (p) => ((p.tasks as { id: string }[]) ?? []).map((t) => t.id),
  press: "b",
  lit: ["a"],
});
