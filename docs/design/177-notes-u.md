# 177 — worker U notes (desktop UI)

Branch `fix/177-u`, on E's contract commit `28935b0e` (base `b540592e`).
Design: `177-local-runtimes.md` §Contract (E) and §UI.

## Decisions (ambiguities, simpler option kept)

1. **Where Local servers lives.** Inside the one Providers section (162 D2),
   between the built-ins and the custom providers, as two sub-headings
   "Local servers" / "Custom providers". A provider with `local: true` (preset
   or local URL, also one added through the custom form) is listed under Local
   servers with the "local" tag; the others stay under Custom providers.
2. **Probe rows.** Every row has Add (an unreachable one too: the server may be
   started later); `added_as` replaces it with "Added as <name>". An unreachable
   row reads "not running" (no "0 models"), its `error` on hover. Add fills the
   form with the row's runtime, that runtime's preset name and the probed
   address (not the preset's default port), and focuses the name. A successful
   add re-runs the probe when results are shown, so the row turns "Added as".
3. **Presets.** Names: `ollama`, `lmstudio`, `omlx`, `llamacpp`, `vllm`; "Other
   OpenAI-compatible" (`openai`) suggests `local` and no address. Switching the
   runtime refills name and address and keeps a typed key.
4. **Keys.** The local form always sends `runtime`; an empty key is `null`. The
   custom form sends 3 arguments (no runtime), `null` for an empty key, and
   allows an empty key only when `isLocalUrl(url)` — E's `local::private_url`
   mirrored exactly (loopback, RFC 1918, CGNAT, link-local, unspecified, ULA,
   v4-mapped v6, single-label hosts, `.localhost .local .internal .lan
   .home.arpa .ts.net`). The help text names the short list (loopback, private,
   .local, .lan).
5. **Limits editor.** Shown for every non-built-in row (custom and local) behind
   a **Limits** toggle. Fields are empty while 0 (unknown); Save sends what is
   typed (an emptied field = `null`), enabled only when a value changed and both
   fields are whole numbers ≥ 1 or empty; Clear sends `null, null`. The reply
   replaces the row in the `provider_models` cache and `list_models` is
   refetched (the pickers show the context). The cache key sits under
   `providers`, so add/refresh/remove refetch it.
6. **"Applies to new chats" not shown.** E's notes do not say running chats
   miss new limits (E6: `set_model_limits` reloads the catalog and re-resolves
   the compactors), so the brief's condition is not met.
7. **Local tags.** A model row, favourite row, search hit and Current row whose
   `ModelInfo.local` is true get the tag, and so does a provider row whose models
   are local; the `ModelCombobox` trigger tags a local current model.
   `localFirst` (summary pickers only) is a stable partition: local providers
   before the others (each part A→Z), local search hits before the others (rank
   kept within each). The chat picker keeps its order.
8. **Test summary call target.** Settings › Memory tests the model a summary
   would use now: `agent.endless.model`, else the title model; with neither the
   button is disabled and the row says each chat uses its own model. The Memory
   panel shows the button in the failing block only (on `stalled.model`): it fit
   there and is where it helps; the header row had no room. Errors are in the
   reply (contract); an RPC error toasts.
9. **`/provider`.** `add <name> <url> [key]`: a missing key is `null` and the
   engine's refusal of a cloud URL ("api key is empty") surfaces verbatim; a
   bare `add` or missing url opens Connections as before. `find` toasts one
   line per probed port. `limits <provider> <model> <context> [output]`: `0` =
   unknown (the engine's own wording); an omitted output keeps the stored one
   (read with `provider_models`) instead of clearing it.
10. **Fixture.** `dev/local-fixture.ts`, chained after the 137 fixture in
    `useApi` (dev only). It adds Sky's `mlx` omlx provider and its models to the
    engine's lists and answers the four 177 methods; `set_model_limits` on `mlx`
    keeps its state until reload. Probe: Ollama :11434 reachable with 3 models,
    LM Studio :1234 not running, :8000 closed, omlx :8080 added as `mlx`.

## Visual check

No scratch engine was built (UI-only change; the fixture answers the 177
methods). The shots render the real `SettingsDialog` (Connections, Memory) and
the chat `ModelMenu` in a throwaway vite page over a fake `Api` with the local
fixture on; the page was removed afterwards. PNGs in `/tmp/177-u/shots/`.
