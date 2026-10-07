---
name: apps
description: How to build an interactive app with the `app` tool — OpenUI Lang syntax, the complete component catalog with positional signatures, state/Query/Mutation/actions, worked prototype and live-service examples, backend choices (installed tools, http_request, an MCP integration), atomic edits and error correction. Load before writing or fixing an `app`.
---

# Apps: clickable screens the user can actually use

`app {name, source}` publishes an OpenUI Lang program as an interactive screen. It renders
progressively on desktop/web while you write it; the user opens it from a card, clicks, types,
and it reads live data through installed tools without another model turn. Use it when the
answer is a screen the user works *in*: a dashboard, a control panel, a form, a prototype UI.

Not an app: one number or paragraph (say it); a static picture (surface fence);
a document, table dump or image (`show`). Apps do not run in text channels; the card there is a
receipt plus source. Library `{{hash}}`.

## An inline fence in a reply

An ```` ```openui ```` fence in a reply renders inline in the message flow, exactly like a
json-render block: no card, no title bar, the reader hovers for "Open as app" and "Show source".
It is static: `$state`, `@Set`/`@Reset`/`@OpenUrl` and inputs work; `Query`, `Mutation` and
`@ToAssistant` are inert until the user opens it as an app. Reach for the explanatory components:
`Steps`/`StepsItem`, `Tabs`/`TabItem`, `Callout`, `Card`/`CardHeader`, `Table`/`Col`,
`BarChart`/`LineChart`/`Series`, `MetricIndicatorInline`, `ListBlock`/`ListItem`, `Tag`,
`Accordion`/`AccordionItem`, `CodeBlock`, `Chips`/`ChipItem`, `OptionCards`/`OptionCard`, and
`Buttons` with `@Set`/`@OpenUrl`. Use the `app` tool instead when the screen needs live data or
actions that run tools.

## Syntax in one screen

```openui
root = Stack([title, body])                 # statements are `name = expression`; `root` is required
$count = 0                                  # `$name` declares reactive state with a literal default
title = TextContent("## Counter")          # components take POSITIONAL arguments, in signature order
body = Stack([label, controls], "row")     # optional arguments follow required ones; skip with null
label = TextContent("Count: " + $count)    # expressions: + - * / comparisons, ?: ternary, &&, ||, !
controls = Buttons([inc, reset])
inc = Button("Add one", Action([@Set($count, $count + 1)]))
reset = Button("Reset", Action([@Reset($count)]), "secondary")
```

- Write `root` first, then the statements it references; reference before definition is fine.
- Strings use double quotes; `null` skips an optional argument; lists are `[a, b]`, objects `{k: v}`.
- Anything not reachable from `root` is never rendered (the receipt lists it).
- `TextContent` renders markdown: headings, **bold**, links `[text](https://…)`, images
  `![alt](https://…)`. External HTTP(S) images and links are normal, allowed functionality.
- Component arguments are checked against the signatures below; a wrong position with a
  compatible type is silently wrong, so use the names as a guide to position.

## Data and actions

```openui
health = Query("argo_app_health", {name: $app}, {status: "Loading", synced: false}, 30)
sync   = Mutation("argo_sync", {name: $app})
btn    = Button("Sync " + $app, Action([@Run(sync), @Run(health)]))
result = TextContent(sync.status == "success" ? sync.data.message : (sync.status == "error" ? sync.error : ""))
```

- `Query(tool, args, defaults, refreshSeconds?)` reads on open, when `args` change, and every
  `refreshSeconds` (minimum 5). Its value is the tool's structured result; `defaults` show
  until the first read lands and are labelled loading, never as live data. Use only tools that
  are installed and visible now; the receipt names each with its risk.
- `Mutation(tool, args)` runs **only** inside an activated action (`@Run`). Never on a timer.
  Results expose `.status` (`"idle" | "loading" | "success" | "error"`), `.data`, `.error`.
- `Action([steps])`: `@Run(mutationOrQuery)`, `@Set($var, value)`, `@Reset($var…)`,
  `@OpenUrl(url)`, `@ToAssistant(message, context?)`. `@Run(mutation)` awaits and stops the
  action on failure; `@Run(query)` starts a refetch and continues.
- **The press is the approval.** One click runs the chosen action, including destructive or
  secret-bearing calls, and the harness shows no second confirmation. Make the consequence clear
  in the label and use `type: "destructive"` on buttons that delete or overwrite. Do not build
  a confirm step; the user pressed the button.
- A tool the policy denies is unavailable in the app; a tool the policy would ask about becomes a
  Load/Refresh affordance whose press is the approval. Do not work around either.
- `@ToAssistant` sends text (and optional context such as a query result) to you as a new user
  input with app provenance. Use it for work that needs the model, not for deterministic reads.
- Form values: `Input("email", "you@example.com", "email", null, $email)` binds the field to
  `$email` (the `value` position); read `$email` in Mutation args. Inputs driving an action are
  captured at activation.

## Where the data comes from

Pick the first that fits; a "prototype" may still talk to real services.

1. State and literals: no tool needed.
2. An installed native/MCP tool, used directly in `Query`/`Mutation`. Reshape with builtins
   (`@Filter`, `@Sort`, `@Count`, field access); several reads need no wrapper.
3. `http_request {url, method, headers?, body?}` → `{status, headers, body}` for a plain REST
   endpoint (1 MiB, 30 s; secrets as `{{secret:NAME}}` placeholders in headers). It is
   `mutating`, so put it behind a button unless the policy allows it automatically.
4. A normal MCP integration when you need joins, multi-step transactions or custom logic: write
   it under `mcp/repos/<name>/` with the language's MCP SDK, test it with the shell, install with
   `add_mcp_server {command, args, cwd}`. Its tools then appear like any other. Give them
   collision-resistant names and describe exactly what a mutation does; the app's metadata pins
   the tool identity and the entry-point source for inspection.
5. `@ToAssistant` for what genuinely needs the model.

Python is your working memory for exploring data; it is not an app backend and cannot be
called from an app.

## Publish, edit, fix

- `app {name: "argo-health", source}` → receipt with `record_call_id`, version, statement /
  query / mutation counts and the bound tools. Say one sentence; the card carries the screen.
- `app {name, edit: true, base_call_id: <record_call_id>, source: <patch>}` replaces or adds
  only the statements in `source`; `name = null` deletes one. The merge is atomic against that
  base; a conflict tells you the current latest record to edit instead. The open panel adopts
  the new version; user-edited state survives for unchanged `$state` declarations.
- Errors come back located: `[code] statement: message`, with the signature of any component
  named. Fix those statements and call again; a first publish that failed has no base, so send
  the complete source again.
- Limits: 128 KiB source, 512 statements, nesting depth 32, 2048 rendered nodes; tool
  responses over 1 MiB fail visibly.

## Examples

Prototype with state, a link and an external image (no tools):

```openui
root = Stack([title, status, picture, controls])
$stage = "idle"
title = TextContent("## DR cutover drill")
status = TextContent("Stage: " + $stage)
picture = TextContent("![Architecture](https://www.rust-lang.org/logos/rust-logo-128x128.png)")
controls = Stack([begin, reset, docs], "row")
begin = Button("Begin drill", Action([@Set($stage, "cutover")]))
reset = Button("Reset", Action([@Reset($stage)]))
docs = Button("Open runbook", Action([@OpenUrl("https://argo-cd.readthedocs.io/")]))
```

Live panel over installed tools `argo_app_health({name})` and `argo_sync({name})`:

```openui
root = Stack([title, pick, healthText, controls, resultText])
$app = "portal-web"
title = TextContent("## ArgoCD health")
pick = Select("app", [SelectItem("portal-web", "Portal web"), SelectItem("portal-api", "Portal API")], "Choose application", null, $app)
health = Query("argo_app_health", {name: $app}, {status: "Loading", synced: false}, 30)
healthText = TextContent($app + ": " + health.status + (health.synced ? " — synced" : " — not synced"))
syncResult = Mutation("argo_sync", {name: $app})
controls = Stack([syncBtn, explain], "row")
syncBtn = Button("Sync " + $app, Action([@Run(syncResult), @Run(health)]))
explain = Button("Explain health", Action([@ToAssistant("Explain this application's health", health.status)]))
resultText = TextContent(syncResult.status == "success" ? syncResult.data.message : (syncResult.status == "error" ? syncResult.error : ""))
```

Table from a query, with a row action:

```openui
root = Card([header, table])
header = CardHeader("Open incidents", @Count(incidents.rows) + " open")
incidents = Query("list_incidents", {status: "open"}, {rows: []}, 60)
ack = Mutation("ack_incident", {id: $ackId})
$ackId = ""
table = Table([
  Col("Title", incidents.rows.title),
  Col("Severity", incidents.rows.severity),
  Col("", @Each(incidents.rows, "i", Button("Acknowledge", Action([@Set($ackId, i.id), @Run(ack), @Run(incidents)]))), "action")
])
```

Step-through diagram (design 65 §3.5): a `Graph` whose `highlight` follows `$step`; pressing a node runs
`onSelect` with `$value` bound to the node id (only `@Set` reads it at press time — `@ToAssistant` and
`@OpenUrl` arguments are evaluated when the props are, so send the selection through a `$state`):

```openui
root = Stack([title, graph, narrative, nav])
$step = 0
$selected = ""
title = TextContent("## One request")
steps = [{ids: ["client"], text: "The client sends the request."}, {ids: ["client", "api"], text: "The API authenticates it."}, {ids: ["api", "db"], text: "The API reads the record."}]
nodes = [{id: "client", label: "Client", kind: "actor"}, {id: "api", label: "API", kind: "rounded"}, {id: "db", label: "Postgres", kind: "cylinder"}]
edges = [{from: "client", to: "api", label: "HTTPS"}, {from: "api", to: "db", label: "reads"}]
graph = Graph(nodes, edges, null, "LR", steps[$step].ids, Action([@Set($selected, $value), @ToAssistant("Explain the selected node", $selected)]))
narrative = TextContent(steps[$step].text + ($selected == "" ? "" : " Selected: " + $selected))
nav = Buttons([Button("Back", Action([@Set($step, $step > 0 ? $step - 1 : 0)]), "secondary"), Button("Next", Action([@Set($step, $step < 2 ? $step + 1 : 2)]))])
```

### When a diagram is an app

A diagram is an app when live data feeds its props (`Query`), a press must do something
(`Action`), or the reader steps through states (`$step` → `highlight`). Static picture: a
`json-render` fence (the `visualize` skill). Reader flips `Tabs`/`Steps` locally with no live
data: an `openui` fence. The diagram components take the json-render props positionally in
catalog order, then `highlight` (unit ids, or indices as strings where the catalog says so) and
`onSelect`. OpenUI Lang has no map: the tool returns rows already in the component's shape (an
MCP tool of yours, item 4 above — `task_inbox` and `schedule_status` are line-oriented today).
`BarChart`/`LineChart`/`ScatterChart`/`RadarChart` here are the upstream recharts charts, not the
json-render grammars of the same names.

Routines as a `Gantt` with Run-now; `plan_status` returns `{tasks: [{id, label, start, end, state}], today, running: [id]}`:

```openui
root = Stack([plan, controls])
$pick = "nightly-digest"
st = Query("plan_status", {}, {tasks: [], today: "2026-10-07", running: []}, 30)
fire = Mutation("schedule_fire", {name: $pick})
plan = Gantt(st.tasks, st.today, "Routines", st.running)
controls = Stack([pick, run], "row")
pick = Select("routine", @Each(st.tasks, "t", SelectItem(t.id, t.label)), "Routine", null, $pick)
run = Button("Run " + $pick + " now", Action([@Run(fire), @Run(st)]))
```

A task's lifecycle with the live state highlighted; `task_state` returns `{state, title}`:

```openui
root = Stack([machine, explain])
task = Query("task_state", {id: "01a1"}, {state: "todo", title: ""}, 15)
machine = StateMachine(states, transitions, "LR", "Task lifecycle", [task.state])
states = [{id: "todo", label: "Todo", kind: "initial"}, {id: "doing", label: "Doing"}, {id: "blocked", label: "Blocked", state: "muted"}, {id: "done", label: "Done", kind: "final"}]
transitions = [{from: "todo", to: "doing", label: "start"}, {from: "doing", to: "blocked", label: "wait"}, {from: "blocked", to: "doing", label: "unblock"}, {from: "doing", to: "done", label: "finish"}]
explain = Button("Why is it " + task.state + "?", Action([@ToAssistant("Explain why this task is in state " + task.state, task)]))
```

Spend by model → session as a `Sankey`; `session_cost` returns `{nodes: [{id, label, stage}], flows: [{from, to, value}]}`:

```openui
root = Sankey(cost.nodes, cost.flows, "USD", "Spend by model and session")
cost = Query("session_cost", {}, {nodes: [], flows: []}, 60)
```

The inbox as a `Swimlane` by state with a move per card; `task_board` returns `{steps: [{id, lane, label}], blocked: [id]}`:

```openui
root = Stack([board, controls])
$card = ""
inbox = Query("task_board", {}, {steps: [], blocked: []}, 15)
start = Mutation("task_move", {task: $card, state: "doing"})
board = Swimlane(lanes, inbox.steps, [], "TB", "Inbox", inbox.blocked)
lanes = [{id: "todo", label: "Todo"}, {id: "doing", label: "Doing"}, {id: "blocked", label: "Blocked"}]
controls = Stack([pick, move], "row")
pick = Select("task", @Each(@Filter(inbox.steps, "lane", "==", "todo"), "t", SelectItem(t.id, t.label)), "Task", null, $card)
move = Button("Start " + $card, Action([@Run(start), @Run(inbox)]))
```

## Catalog

{{catalog}}
