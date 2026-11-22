/**
 * One valid source per diagram type mermaid 12.0.0 registers (dist/mermaid.core.mjs `addDiagrams` +
 * `registerLazyLoadedDiagrams`, detector ids). `corpus.ts` derives the invalid variants from these.
 */
export const VALID: Record<string, string> = {
  flowchart: `flowchart TD
  A[Start] --> B{Is it?}
  B -->|Yes| C[OK]
  B -->|No| D[End]`,
  graph: `graph LR
  A --> B
  B --> C`,
  "flowchart-elk": `flowchart-elk TD
  A --> B
  B --> C`,
  sequence: `sequenceDiagram
  participant A as Alice
  participant B as Bob
  A->>B: Hello
  B-->>A: Hi`,
  classDiagram: `classDiagram
  class Animal {
    +String name
    +eat() void
  }
  Animal <|-- Dog`,
  stateDiagram: `stateDiagram-v2
  [*] --> Still
  Still --> Moving
  Moving --> [*]`,
  "stateDiagram-v1": `stateDiagram
  [*] --> Still
  Still --> [*]`,
  er: `erDiagram
  CUSTOMER ||--o{ ORDER : places
  ORDER ||--|{ LINE_ITEM : contains`,
  gantt: `gantt
  title A Gantt
  dateFormat YYYY-MM-DD
  section S1
  Task A :a1, 2024-01-01, 30d
  Task B :after a1, 20d`,
  pie: `pie title Pets
  "Dogs" : 386
  "Cats" : 85`,
  journey: `journey
  title My day
  section Go to work
    Make tea: 5: Me
    Go upstairs: 3: Me, Cat`,
  gitGraph: `gitGraph
  commit
  branch develop
  checkout develop
  commit
  checkout main
  merge develop`,
  mindmap: `mindmap
  root((mindmap))
    Origins
      Long history
    Research`,
  timeline: `timeline
  title History
  2002 : LinkedIn
  2004 : Facebook : Google`,
  quadrantChart: `quadrantChart
  title Reach
  x-axis Low Reach --> High Reach
  y-axis Low Engagement --> High Engagement
  quadrant-1 Expand
  quadrant-2 Promote
  quadrant-3 Re-evaluate
  quadrant-4 Improve
  Campaign A: [0.3, 0.6]`,
  xychart: `xychart-beta
  title "Sales"
  x-axis [jan, feb, mar]
  y-axis "Revenue" 4000 --> 11000
  bar [5000, 6000, 7500]
  line [5000, 6000, 7500]`,
  sankey: `sankey-beta
A,B,10
B,C,5`,
  requirement: `requirementDiagram
  requirement test_req {
    id: 1
    text: the test text.
    risk: high
    verifymethod: test
  }
  element test_entity {
    type: simulation
  }
  test_entity - satisfies -> test_req`,
  c4: `C4Context
  title System Context
  Person(customer, "Customer", "A customer")
  System(sys, "System", "The system")
  Rel(customer, sys, "Uses")`,
  block: `block-beta
  columns 3
  a b c
  d["wide"]:2 e`,
  packet: `packet-beta
  0-15: "Source Port"
  16-31: "Destination Port"`,
  architecture: `architecture-beta
  group api(cloud)[API]
  service db(database)[Database] in api
  service server(server)[Server] in api
  db:L -- R:server`,
  kanban: `kanban
  todo[Todo]
    t1[Write docs]
  done[Done]
    t2[Ship]`,
  radar: `radar-beta
  axis A, B, C
  curve c1{1, 2, 3}`,
  treemap: `treemap-beta
"Root"
    "A": 10
    "B": 20`,
  info: `info`,
  ishikawa: `ishikawa-beta
  Problem
    Cause A
      Sub A
    Cause B`,
  venn: `venn-beta
  set A
  set B
  union A,B`,
  wardley: `wardley-beta
  title Tea
  component Tea [0.8, 0.3]
  component Water [0.4, 0.8]
  Tea -> Water`,
  cynefin: `cynefin-beta
  title Decisions`,
  usecase: `usecase-beta
  actor User
  User --> "Login"`,
  railroad: `railroad-beta
  rule = choice(terminal("a"), terminal("b"));`,
  railroadEbnf: `railroad-ebnf-beta
  rule = "a" | "b" ;`,
  railroadAbnf: `railroad-abnf-beta
  rule = "a" / "b";`,
  railroadPeg: `railroad-peg-beta
  rule <- "a" / "b";`,
  agentflow: `agentflow-beta
  A --> B`,
  swimlane: `swimlane-beta
  A --> B`,
  treeView: `treeView-beta
  root
    child`,
  eventmodeling: `eventmodeling
  tf 01 cmd Order`,
};
