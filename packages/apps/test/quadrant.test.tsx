/** 123 W2: `Quadrant` parity (see `chart-parity.tsx`); unit ids are item ids (the label when an item has none). */
import { parity } from "./chart-parity.tsx";

parity({
  name: "Quadrant",
  prefix: "quadrant",
  app: `root = Stack([q, out])
$lit = ["cache"]
$selected = ""
$value = ""
q = Quadrant("Effort", "Impact", [{id: "cache", label: "Cache", x: 0.2, y: 0.8}, {id: "rewrite", label: "Rewrite", x: 0.9, y: 0.7}, {label: "Tests", x: 0.5, y: 0.6}], ["Do first", "Plan", "Fill-ins", "Avoid"], ["low", "high"], ["low", "high"], "What first", $lit, Action([@Set($selected, $value)]))
out = TextContent("Selected: " + $selected)`,
  units: (p) => ((p.items as { id?: string; label: string }[]) ?? []).map((it) => it.id ?? it.label),
  press: "Tests",
  lit: ["cache"],
});
