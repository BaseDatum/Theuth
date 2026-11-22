/**
 * The mermaid parity corpus: for every registered diagram type one valid source, a syntax error on line 2
 * and a syntax error on the last line; plus edge cases (empty, whitespace, unknown keyword, front matter,
 * `%%{init}%%`, HTML labels, unicode, a 32 KB diagram = the reply-fence cap).
 */
import { VALID } from "./valid.ts";

export interface Case {
  name: string;
  type: string;
  source: string;
}

const GARBAGE = "}}} ]]] ((( @@@ !!! <<<";
/** Grammars that swallow GARBAGE as free text get a line they do reject (found by probing the jsdom path). */
const BAD_LINE: Record<string, string> = {
  stateDiagram: "[*] --> --> [*]",
  "stateDiagram-v1": "[*] --> --> [*]",
  timeline: ":::",
  treeView: ":::",
};

/** ~32 KB flowchart; `label` pads each line so the edge count stays under mermaid's 500-edge limit. */
function big(label = ""): string {
  const lines = ["flowchart TD"];
  let i = 0;
  while (lines.join("\n").length < 32 * 1024 - 40) {
    lines.push(`  n${i}[Node ${i}${label}] --> n${i + 1}[Node ${i + 1}]`);
    i++;
  }
  return lines.join("\n");
}

export function corpus(): Case[] {
  const out: Case[] = [];
  for (const [type, src] of Object.entries(VALID)) {
    const lines = src.split("\n");
    out.push({ name: `${type}/valid`, type, source: src });
    out.push({
      name: `${type}/invalid-early`,
      type,
      source: [lines[0], BAD_LINE[type] ?? GARBAGE, ...lines.slice(1)].join("\n"),
    });
    out.push({ name: `${type}/invalid-late`, type, source: `${src}\n${BAD_LINE[type] ?? GARBAGE}` });
  }
  const edge: [string, string][] = [
    ["empty", ""],
    ["whitespace", "  \n\t\n  "],
    ["unknown-keyword", "notADiagram\n  A --> B"],
    ["error-keyword", "error"],
    ["front-matter", "---\ntitle: Hello\nconfig:\n  theme: forest\n---\nflowchart LR\n  A --> B"],
    ["front-matter-invalid-body", "---\ntitle: Hello\n---\nflowchart LR\n  A --> B\n  " + GARBAGE],
    ["front-matter-unclosed", "---\ntitle: Hello\nflowchart LR\n  A --> B"],
    ["front-matter-bad-yaml", "---\ntitle: [unclosed\n---\nflowchart LR\n  A --> B"],
    ["init-directive", "%%{init: {'theme':'dark'}}%%\nsequenceDiagram\n  A->>B: hi"],
    ["init-directive-bad-json", "%%{init: {'theme': }}%%\nsequenceDiagram\n  A->>B: hi"],
    ["comment-lines", "%% a comment\nflowchart LR\n  %% another\n  A --> B"],
    ["html-label", 'flowchart LR\n  A["<b>bold</b> <i>it</i><br/>line"] --> B["<script>alert(1)</script>"]'],
    ["html-label-img", 'flowchart LR\n  A["<img src=x onerror=alert(1)>"] --> B'],
    ["html-sequence", "sequenceDiagram\n  A->>B: <b>hi</b><br/>there"],
    ["unicode", "flowchart LR\n  A[日本語 ✓] --> B[Ünïcödé 🎉]\n  B --> C[עברית]"],
    ["unicode-sequence", "sequenceDiagram\n  participant 甲\n  甲->>乙: こんにちは 👋"],
    ["crlf", "flowchart LR\r\n  A --> B\r\n  B --> C"],
    ["leading-blank-lines", "\n\n\nflowchart LR\n  A --> B\n  " + GARBAGE],
    ["big-32k", big()],
    ["big-32k-error-at-end", `${big().slice(0, 32 * 1024 - 60)}\n  ${GARBAGE}`],
    ["big-32k-under-edge-limit", big(" padding padding padding padding padding padding")],
    ["big-32k-under-edge-limit-error-at-end", `${big(" padding padding padding padding padding padding")}\n  ${GARBAGE}`],
    // A TypeError inside the timeline db: the message is the JS engine's own wording.
    ["timeline-typeerror", "timeline\n: x"],
    ["ishikawa-lenient", `ishikawa-beta\n  Problem\n${GARBAGE}`],
    ["classDef-style", "flowchart LR\n  A:::hot --> B\n  classDef hot fill:#f96,stroke:#333\n  style B fill:#bbf"],
    ["click-callback", 'flowchart LR\n  A --> B\n  click A callback "Tooltip"\n  click B "https://example.com" _blank'],
    ["subgraph", "flowchart TB\n  subgraph one\n    a1 --> a2\n  end\n  subgraph two\n    b1 --> b2\n  end\n  one --> two"],
    ["sequence-loop-unclosed", "sequenceDiagram\n  loop Every minute\n    A->>B: ping"],
    ["gantt-bad-date", "gantt\n  dateFormat YYYY-MM-DD\n  section S\n  Task :a1, notadate, 3d"],
    ["pie-negative", 'pie\n  "A" : -5'],
    ["accessibility", "flowchart LR\n  accTitle: Title\n  accDescr: Description\n  A --> B"],
  ];
  for (const [name, source] of edge) out.push({ name: `edge/${name}`, type: "edge", source });
  return out;
}
