---
root: /Users/Sky.Moore/Develop/theuth-71-s9
---
# How a popout window boots

## What and why

A popout is a second desktop window showing one chat, artifact, note, meeting or terminal — the same React bundle at the same route with `?window=<label>` in the URL. The label is what Tauri keys the window on (focus-if-open, remembered size/position), and in a plain browser it becomes the `window.open` name. Popouts boot by parsing the URL query once at bundle load, then rendering a minimal layout (the column alone, no sidebar) driven by that frozen target.

## Map

The mechanism spans three files: the URL parser that extracts the target, the module-level constant that captures it once, and the root route that switches layout based on that constant.

```json-render
{
  "root": "graph",
  "elements": {
    "graph": {
      "type": "Graph",
      "props": {
        "direction": "TB",
        "nodes": [
          {"id": "url", "label": "URL with ?window=", "kind": "box", "ref": "crates/desktop/ui/src/lib/popout.ts#L49"},
          {"id": "parser", "label": "popoutTarget", "ref": "crates/desktop/ui/src/lib/popout.ts#L52-L66"},
          {"id": "constant", "label": "POPOUT constant", "ref": "crates/desktop/ui/src/lib/popout.ts#L69"},
          {"id": "app", "label": "App.tsx AUX flag", "ref": "crates/desktop/ui/src/App.tsx#L51"},
          {"id": "root", "label": "__root.tsx route", "ref": "crates/desktop/ui/src/routes/__root.tsx#L16"},
          {"id": "popped", "label": "Popped component", "ref": "crates/desktop/ui/src/routes/__root.tsx#L20-L28"}
        ],
        "edges": [
          {"from": "url", "to": "parser", "label": "parses"},
          {"from": "parser", "to": "constant", "label": "initializes"},
          {"from": "constant", "to": "app", "label": "sets AUX"},
          {"from": "constant", "to": "root", "label": "picked by"},
          {"from": "root", "to": "popped", "label": "renders"}
        ]
      }
    }
  }
}
```

## How it works

The flow begins when `openPopout` is called from a button or menu item. It either invokes the Tauri command `open_window` or calls `window.open` with the [popout URL](code:crates/desktop/ui/src/lib/popout.ts#L49) and [label](code:crates/desktop/ui/src/lib/popout.ts#L47). The new window loads the same bundle, and [popoutTarget](code:crates/desktop/ui/src/lib/popout.ts#L52-L66) parses `location.href` to extract the target from the `?window=` query parameter.

```json-render
{
  "root": "seq",
  "elements": {
    "seq": {
      "type": "Sequence",
      "props": {
        "title": "Popout boot sequence",
        "actors": [
          {"id": "user", "label": "User"},
          {"id": "open", "label": "openPopout"},
          {"id": "window", "label": "New window"},
          {"id": "parser", "label": "popoutTarget"},
          {"id": "constant", "label": "POPOUT"},
          {"id": "router", "label": "Router"},
          {"id": "popped", "label": "Popped"}
        ],
        "steps": [
          {"from": "user", "to": "open", "label": "clicks button", "ref": "crates/desktop/ui/src/lib/popout.ts#L89-L93"},
          {"from": "open", "to": "window", "label": "invoke open_window or window.open", "ref": "crates/desktop/ui/src/lib/popout.ts#L90-L91"},
          {"from": "window", "to": "parser", "label": "loads bundle, parses location.href", "ref": "crates/desktop/ui/src/lib/popout.ts#L52-L66"},
          {"from": "parser", "to": "constant", "label": "returns PopoutTarget", "ref": "crates/desktop/ui/src/lib/popout.ts#L69"},
          {"from": "constant", "to": "router", "label": "isPopout() true, picks Popped", "ref": "crates/desktop/ui/src/routes/__root.tsx#L16"},
          {"from": "router", "to": "popped", "label": "renders column alone", "ref": "crates/desktop/ui/src/routes/__root.tsx#L20-L28"}
        ]
      }
    }
  }
}
```

The [POPOUT constant](code:crates/desktop/ui/src/lib/popout.ts#L69) is initialized once at module load, reading `location.href` before any route navigation can strip the query parameter. This value stays frozen for the window's lifetime. [App.tsx](code:crates/desktop/ui/src/App.tsx#L51) checks it to set the `AUX` flag, which disables the main shell's singletons (meeting listeners, dictation, global shortcuts). The [root route](code:crates/desktop/ui/src/routes/__root.tsx#L16) picks the component: `Outlet` for the island webview, `Popped` when `isPopout()` is true, otherwise `Root` with the full sidebar.

```json-render
{
  "root": "ex1",
  "elements": {
    "ex1": {
      "type": "Excerpt",
      "props": {
        "path": "crates/desktop/ui/src/lib/popout.ts",
        "text": "/** Read once: the route may drop the query on its first navigation. */\nexport const POPOUT: PopoutTarget | null = typeof location === 'undefined' ? null : popoutTarget(location.href)",
        "start": 68,
        "caption": "The constant reads location.href once at bundle load"
      }
    }
  }
}
```

```json-render
{
  "root": "ex2",
  "elements": {
    "ex2": {
      "type": "Excerpt",
      "props": {
        "path": "crates/desktop/ui/src/routes/__root.tsx",
        "text": '/** 03 §0.1: the island webview gets no sidebar, search or hotkeys — just its route. A popout gets the routed column alone. */\nexport const Route = createRootRoute({ component: isIsland() ? Outlet : isPopout() ? Popped : Root })',
        "start": 15,
        "caption": "The route picks component based on isPopout()"
      }
    }
  }
}
```

```json-render
{
  "root": "ex3",
  "elements": {
    "ex3": {
      "type": "Excerpt",
      "props": {
        "path": "crates/desktop/ui/src/routes/__root.tsx",
        "text": '/** One chat or one artifact in its own window: the column without sidebar, search, drag targets or the meeting bar.\n * `SidebarProvider` stays (closed) because the chat header reads it. */\nfunction Popped() {\n  return (\n    <SidebarProvider open={false} className="h-band min-h-0">\n      <SidebarInset className="min-h-0 overflow-hidden pt-[var(--safe-top)] pr-[var(--safe-right)] pb-[var(--safe-bottom)] pl-[var(--safe-left)]">\n        <Outlet />\n      </SidebarInset>\n    </SidebarProvider>\n  )',
        "start": 18,
        "caption": "Popped renders the column without sidebar or meeting bar"
      }
    }
  }
}
```

## Decisions and trade-offs

The module-level constant (read once at load) avoids re-parsing the URL on every render and ensures the target stays stable even if the route navigates and strips the query. The trade-off: a popout cannot change what it shows — to pop out a different chat, you close the window and open a new one. The [same bundle serves both shells](code:crates/desktop/ui/src/App.tsx#L48-L51), so every popout shares the main window's code, styles and React version without duplication.
