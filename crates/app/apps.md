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

## Catalog

{{catalog}}
