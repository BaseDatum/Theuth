# herdr — THEIRS-SURFACE (A5–A9)

_Recon child `herdr-theirs-surface`, 2026-10-09. Source: shallow clone /tmp/herdr-recon (HEAD 2563803d, v0.9.3). Read-only; cites `path#La-Lb` relative to that root. Status: complete. 290+ cites; every cited range was re-read in a verification pass (2 skill-file ranges and 1 socket-api line were corrected). No code copied; techniques only._

## Summary

- **A5** — five agent states with a per-client "done/seen" bit; the server emits one semantic notification (`needs attention | finished | updated`, with pane/tab/workspace ids and a sound) only on blocked or background-completion transitions, never for the active tab; each client owns delivery (in-app toast, outer-terminal OSC 9/99 with tmux passthrough, OS notifier, embedded mp3), with a state-rechecking delay, per-pane replacement and a queue of 8; the sidebar is a declarative token grid that agents can feed via `report-metadata`.
- **A6** — one `ssh host exec herdr remote-client-bridge` per machine relays bytes to a 0600 local socket; capabilities (`surface_interest`, `health_check`, endpoint generation) decide keep/handoff/stop instead of version equality; only the viewed machine streams screens; backoff resets after 60 s healthy; cached state dims, never steals selection; auth stays in OpenSSH, the binary is streamed over the same ssh and committed with `mv`.
- **A7** — a Zig-built vendored libghostty-vt behind a 5 K-line FFI replaced vt100 in 0.4.0; per-client virtual rendering with two base encodings and three frozen optional codecs (reuse / sparse delta / scroll-shift); alt-screen reads harvest TUI history with synthetic wheel events; three portable-pty patches, all Windows (ConPTY DLL loading, raw cmd tails, env block sanitising); patches carry issue, base, verification and removal condition.
- **A8** — worktrees are workspaces with git provenance (`<root>/<repo>/<branch-slug>`, `worktree/<adj>-<noun>-<hex>` branches, never delete the branch, dirty-refusal re-asked, leftover recovery, group-close guard); git metadata read from `.git` files behind a file-stamp dependency cache and one detached worker; plugins are manifest-declared argv processes with the whole CLI as API, no sandbox, trust preview + `min_herdr_version`; marketplace = GitHub topic index.
- **A9** — a 330-line AGENTS.md written for agents (multiplicative-path rule, runtime/client boundary, append-closed frozen codecs, refactor-risk classification, external-contributor guardrail); repo-local skills for triage, throwaway repro and pre-release audit; 3,901 unit + 255 binary-spawning integration tests + Python/Bun architecture and maintenance tests; preview→stable promotion where only version/notes may differ; versioned docs per release; a remote-updated detection catalog; 59 releases in six months.

**Three most borrowable ideas:** (1) the server-decides-*whether* / client-decides-*how* semantic notification with blocked-only + background-completion-only rules, re-check-at-delivery and per-surface seen bits — the direct answer to "which one is blocked"; (2) an engine-managed worktree API with provenance and safe removal, replacing hand-cut wave worktrees; (3) capability-negotiated compatibility with explicit restart reasons and append-closed codecs, so engines, desktop and companion can drift across releases.

## A5 Supervision UX

**One-line pointers to A1–A4 (covered by the sibling):** states come from `src/detect/*` (process/screen/hook arbitration); hook reports arrive over the socket API; persistence in `src/persist/*`. This axis starts where a pane already *has* a state.

### What the user sees for many agents

- **Five states, two of which are the same state seen from two sides.** herdr shows `blocked`, `working`, `done`, `idle`, `unknown`; `done` is "finished and you have not looked at it yet", and the seen-bit is **per client**: "Viewing a completion in one client does not clear another client's Done badge", while CLI/API statuses use the server's seen state (docs/next/website/src/content/docs/concepts.mdx#L41-L51). The headless server computes the pane's *effective* state by looking up the attached terminal (src/server/headless/notifications.rs#L4-L20); the client projects `Idle` into `done`/`idle` from its own seen set (src/app/agent_view.rs#L327-L330 shows the same mapping used for the API view).
- **Status marks are dots by default, symbols on request.** `ui.status_indicators = "dots" | "symbols"` (src/config/model.rs#L113-L125); the doc states the reason for the second style — distinguish states "by shape as well as color" — and the deliberate limit: "The symbols are static, so this option does not enable spinner animation" (configuration.mdx#L353-L360).
- **The sidebar is a declarative token grid, not hard-coded rows.** `[ui.sidebar.agents].rows` is an array of rows of tokens; defaults are `["state_icon","machine","workspace","tab"]` + `["agent"]` for agents and `["state_icon","workspace"]` + `["branch","git_status"]` for spaces (configuration.mdx#L362-L380). Built-in tokens include `state_text`, `terminal_title`, `terminal_title_stripped` (title with one leading spinner glyph removed) and `$name` custom metadata (configuration.mdx#L382-L393). Missing values and their separators disappear, rows with no values disappear, ≤16 rows × 16 tokens (configuration.mdx#L404). Tokens accept inline style tables and up to 16 ordered `rules` (`equals/contains/starts_with/gt/lt`, `hide = true`), first match wins; "regex, fuzzy matching, and scripts are not supported" (configuration.mdx#L406-L435). A per-agent override table `rows_by_agent` replaces the layout for one canonical agent id (configuration.mdx#L439-L456). The resolver is `agent_rows(config, context, state)` producing `ResolvedToken`s with `StateIcon`/`Machine`/`Custom` kinds (src/ui/sidebar/tokens.rs#L240-L262 tests show machine colour/bold/dim per rule).
- **Custom tokens are fed by agents or scripts, styling stays local.** `herdr pane report-metadata <pane_id> --source my-agent-hook --token model=opus --token summary="reviewing authentication"` fills `$model`/`$summary`; "Metadata reporters provide values only; styling stays in the local sidebar configuration" (configuration.mdx#L458-L478).
- **Tab bar right edge is a tmux-style status area** of typed segments (`zoom`, `hostname`, `datetime`, `text`, `command`), resolved **on the server** so `herdr --remote` shows the remote machine's values; `command` runs on an interval without blocking rendering, takes the *last line*, strips ESC-prefixed control sequences "instead of interpreting styles", and clears on failure/timeout (configuration.mdx#L318-L336). The implementation caps command output to 4096 bytes and status text to 80 chars (src/app/tab_bar_status.rs#L15-L17) and strips bidi/zero-width format controls (src/app/tab_bar_status.rs#L241-L251); config reload kills the in-flight command's process group synchronously (src/app/tab_bar_status.rs#L34-L43).
- **Outer window title is rendered by herdr**, because OSC 0/2 from inside a pane "stops at Herdr"; template tokens `{hostname} {workspace} {tab} {pane} {terminal_title}` with spinner frames stripped (configuration.mdx#L338-L351).
- **Goto picker (`prefix+g`)**: every agent/terminal is its own row grouped by workspace, `/` searches names, agent kinds, tabs, workspaces, branches or paths; outside search `b/w/i/d` filter blocked/working/idle/done, `a` shows all; Left/Right jump between workspace sections (keyboard.mdx#L65). `NextAgent`/`PreviousAgent` keybind actions cycle through the agent list modulo its length (src/input/keybindings.rs#L34; src/client/shell/actions.rs#L892-L909), and `focus_agent = "prefix+alt+1..9"` is an indexed jump (configuration.mdx#L177-L188).

### How "blocked" reaches the user

- **Only two transitions notify.** `notification_toast_for_state_change` returns `NeedsAttention` on any transition *into* `Blocked`, `Finished` only on a *background* completion transition into `Idle`, and nothing otherwise — and nothing at all for the active tab (src/app/actions.rs#L77-L93). "Herdr suppresses popups for the active tab" (configuration.mdx#L495).
- **The server emits one semantic event; each client decides delivery.** The headless server packages a `SemanticNotification { kind, title "<agent> needs attention|finished|updated", body, sound: Request|Done|None, agent, workspace_id, tab_id, pane_id, position }` and sends it to the client shells (src/server/headless/notifications.rs#L40-L109; wire struct at src/protocol/wire.rs#L1367-L1377). A doc comment pins the discipline: "ALL internal events MUST be routed through this method to ensure clipboard/notify forwarding is never bypassed" (src/server/headless/notifications.rs#L292-L301); the server "has no host terminal or audio subsystem", so sounds and toasts are forwarded to the **foreground client only**, and clipboard writes too (src/server/headless/notifications.rs#L702-L712, #L335-L336).
- **Delay = debounce against flapping.** `ui.toast.delay_seconds` (≤ 3600, src/config/model.rs#L12) parks a `PendingAgentNotification` keyed by pane with a deadline (src/app/actions.rs#L1897-L1930); at delivery the pane must still be in the expected state with the same agent label or the notification is dropped (src/app/actions.rs#L1941-L1954). A newer transition on the same pane removes the pending one (src/app/actions.rs#L1868).
- **Client-side policy: one visible, a queue of 8, per-pane replacement.** Durations are 8 s for needs-attention, 5 s finished, 3 s update-installed (src/client/shell/notification_policy.rs#L15-L22); at most `MAX_QUEUED_NOTIFICATIONS = 8`, oldest dropped (src/client/shell/notification_policy.rs#L3, #L50-L53); a new event for the same endpoint+pane evicts pending, queued and visible ones for that pane (src/client/shell/notification_policy.rs#L113-L131); `Finished` is "advisory" and only valid while the client-projected pane remains Done, with a 1 s evidence grace (src/client/shell/notification_policy.rs#L4, #L133-L142). Notifications of a disconnected endpoint are retired wholesale (src/client/shell/notification_policy.rs#L25-L38).
- **Focus/jump from the toast.** Acting on the visible notification sends `PaneFocus` on the active endpoint or `ActivateEndpoint { target: Pane }` on another machine; if that endpoint is offline the user is told "<label> is unavailable" instead (src/client/shell/notification_policy.rs#L65-L97).
- **Four delivery channels, chosen by config `ui.toast.delivery = herdr | terminal | system | off`** (configuration.mdx#L482-L499):
  - *terminal*: `TERM_PROGRAM`/`TERM`/`KITTY_WINDOW_ID` pick Ghostty/iTerm2/WezTerm (OSC 9) or kitty (OSC 99 with structured title+body); the sequence is wrapped in tmux passthrough when `$TMUX` is set, and ESC/BEL/ST bytes are stripped from the text (src/terminal_notify.rs#L11-L32, #L34-L56, #L65-L82, #L84-L106). This is what "also works over SSH" means.
  - *system* on macOS: `terminal-notifier` first "because it can activate the hosting terminal on click" (`-activate <bundle id>` of the detected terminal), fallback `/usr/bin/osascript` which "appears as Script Editor" and cannot activate anything (src/platform/macos.rs#L736-L745, #L783-L795; configuration.mdx#L497). On Windows a click selects the originating pane and raises the window when Windows exposes it, but "cannot reopen a closed client or select a pane from a restarted server" (configuration.mdx#L499; src/platform/windows/notifications.rs, 533 lines).
  - *sound*: two mp3s embedded with `include_bytes!` (src/sound.rs#L27-L28), played by `afplay` / Windows MediaPlayer via PowerShell / the first of `paplay, pw-play, ffplay, mpg123, mpv` on Linux — "Do not add bare aplay here. It does not decode MP3" (src/sound.rs#L4, #L120, #L182, #L310-L329); a 15 s player timeout and `HERDR_DISABLE_SOUND` (src/sound.rs#L18-L24). Per-agent sound overrides `default|on|off`, and "Droid is muted by default" (configuration.mdx#L512-L520).
- **In-product release notes and announcements.** Pending release notes are saved to `release-notes.json` beside the config at update time and shown once per version (`mark_current_version_seen`) (src/release_notes.rs#L6, #L27-L33, #L106); product announcements come from the update manifest as `{id, title?, body}` and are stored with a `seen` set keyed by version+id, with `HERDR_FAKE_PRODUCT_ANNOUNCEMENT_*` env hooks for testing (src/product_announcements.rs#L8-L46). Both render as an " ● update ready" style badge in the launcher/settings (src/ui/release_notes.rs#L307).

### Worked trace: one agent becomes blocked on a background tab

1. A detector transition lands as a `PaneStateUpdate` / effective state change; the headless server routes *every* internal event through `handle_internal_event_with_forwarding` (src/server/headless/notifications.rs#L292-L301).
2. `notification_toast_for_state_change(false, prev, Blocked)` → `Some(NeedsAttention)`; the same call with the active tab's suppression flag returns `None` (src/app/actions.rs#L77-L93).
3. With `delay_seconds > 0` the app parks a `PendingAgentNotification {pane_id, workspace_id, agent_label, kind, state, deadline}` (src/app/actions.rs#L1909-L1928); any newer transition on the pane removes it (src/app/actions.rs#L1868).
4. At the deadline, delivery re-reads the terminal: state must still be `Blocked` and the agent label unchanged, else nothing is sent (src/app/actions.rs#L1941-L1954).
5. The server builds `SemanticNotification { kind: NeedsAttention, title: "<agent> needs attention", body: "<workspace · tab>", sound: Request, workspace_id, tab_id, pane_id }` and sends it to the client shells (src/server/headless/notifications.rs#L70-L108; struct src/protocol/wire.rs#L1367-L1377).
6. Each client's `receive_notification` evicts older pending/queued/visible items for the same endpoint+pane, then queues it with an 8 s display budget (src/client/shell/notification_policy.rs#L99-L145, #L15-L22).
7. Delivery per `ui.toast.delivery`: in-app toast; or OSC 9/99 to the outer terminal (tmux-wrapped if needed); or `terminal-notifier -activate <terminal bundle>` / `osascript`; and the embedded `request.mp3` through `afplay` (src/terminal_notify.rs#L34-L56; src/platform/macos.rs#L736-L745; src/sound.rs#L27-L28, #L120).
8. The user acts on the toast: `PaneFocus` on this machine or `ActivateEndpoint { target: Pane }` on another; an offline endpoint yields "<label> is unavailable" (src/client/shell/notification_policy.rs#L65-L97).
9. The sidebar row for that pane shows the blocked `state_icon` (dot or symbol) until the state changes; "done" afterwards is per-client until that client views it (configuration.mdx#L353-L360; concepts.mdx#L51).

History of the surface (CHANGELOG): the expanded agent panel gained an all-workspaces mode and the collapsed sidebar "compact per-pane agent indicators" in 0.4.2 (CHANGELOG.md#L1099-L1103); the first-run onboarding that asks for sound/toast preferences dates from 0.1.2 (CHANGELOG.md#L1246-L1250).

### Keyboard + mouse model

Mouse-native first: click panes/tabs/workspaces/agents, drag split borders, right-click menus; `ui.mouse_capture = false` to hand the mouse back (concepts.mdx#L26-L35). Keyboard is a tmux-style prefix (`ctrl+b`), `prefix+?` shows every active binding with a `/` filter (keyboard.mdx#L12-L18); the doc argues for `ctrl+alt` as the free modifier family for prefix-free chords and lists the chords other software owns (keyboard.mdx#L106-L133). Three modes: terminal, prefix, navigate (concepts.mdx#L83-L89).

### What it deliberately does not show

No spinner animation in status marks (configuration.mdx#L360); no popups for the active tab (configuration.mdx#L495); no notification for `Working→Idle` in the foreground, no notification for `Unknown` (src/app/actions.rs#L86-L92); the mobile/collapsed sidebar keeps a fixed compact layout and ignores row config (configuration.mdx#L480); the status `command` segment shows the *last line only*, styles stripped (configuration.mdx#L334); custom-token rules refuse regex and scripts (configuration.mdx#L435).

### Weaknesses

- Three notification layers (server pending-with-delay, client pending/queued/visible, OS/terminal backend) each with their own dedupe; the 50 ms `COMPLETION_RECHECK_INTERVAL` polling (src/client/shell/notification_policy.rs#L5-L6) exists because completion evidence can arrive after the event.
- Terminal and system backends are detection-by-env-var and external binaries (`terminal-notifier`, `osascript`, `afplay`, `paplay`…): click-to-focus works only on a detected terminal; the osascript fallback cannot focus anything (configuration.mdx#L497).
- Phone supervision is "SSH in and run the TUI" (how-to-work.mdx#L47-L56) — no push, no out-of-window surface; the One product exists to fill exactly this gap.
- A toast queue of 8 with oldest-dropped means a burst of >9 blocked agents silently loses events; there is no inbox to catch up from, only the sidebar state.
- Status marks are a single glyph per agent; "blocked" carries no reason text unless an agent reports `$summary` itself.

### Borrow candidates for theuth

1. **Semantic-notification envelope, server decides *whether*, client decides *how*** — `{kind, title, body, sound, agent, workspace/tab/pane ids}` fan-out to the foreground client only; all internal events through one routing method. **S–M**, lands in the attention inbox (104/132) + crates/rpc event stream + companion (130); matters because Sky's "which one is blocked" problem is a delivery problem, and theuth already has the inbox as the *whether*.
2. **Blocked-only + background-completion-only notification rule, with active-view suppression and a per-item delay that re-checks state before delivering** (src/app/actions.rs#L77-L93, #L1941-L1954). **S**, lands in attention inbox filing; kills flapping notifications from ACP children that block for 300 ms.
3. **Per-client "done/seen" bit** — the server keeps one state; each surface (desktop, phone, Slack) clears its own Done badge (concepts.mdx#L51). **S–M**, lands in crates/rpc + desktop/companion stores.
4. **Terminal-emitted OSC 9/99 notification with tmux passthrough** for the TUI client and for PTY-driven sessions over SSH (src/terminal_notify.rs). **S**, lands in crates/tui; gives notifications on a phone SSH client (Blink/Termius/moshi) for free.
5. **Declarative row/token grid with first-match style rules and agent-reported `$tokens`** for the agents sidebar. **M**, lands in crates/desktop sidebar + crates/tui; lets subagents report `summary`/`model` without a UI change per field.
6. **Goto picker with single-letter state filters (`b/w/i/d`) and workspace-section jumps** (keyboard.mdx#L65). **S**, lands in desktop command palette / TUI menu.
7. **Server-rendered outer window title** `{hostname}: {workspace}` with spinner frames stripped (configuration.mdx#L338-L351). **S**, lands in crates/tui + desktop window title; cheap glanceability in a dock/alt-tab.
8. **`terminal-notifier -activate <bundle>` when present** for click-to-focus on macOS (src/platform/macos.rs#L736-L745). **S**, lands in companion (130) if the companion is not yet a notification sender; otherwise skip.

## A6 Multi-machine

**Pointer to A3/A4:** detach/reattach, live handoff and the socket API are the sibling's; here only what multi-machine adds on top.

### Process model: ssh + a byte bridge, the client never speaks SSH itself

- **One ssh process per machine, running `herdr remote-client-bridge`** on the remote end. The local client spawns `ssh <target> exec herdr --session <name> remote-client-bridge [--idle-timeout-v1]` (src/remote/attach.rs#L397-L425); the bridge on the remote host connects to the remote server's local socket and relays bytes over ssh stdio. Locally, `SshStdioBridge` owns a **local socket file** (mode `0o600`, src/remote/attach.rs#L26) that the ordinary client code connects to as if the server were local (src/remote/attach.rs#L2759-L2789). The module header says it plainly: "Remote thin-client launcher over SSH command stdio" (src/remote/attach.rs#L1). Saved machines use the same bridge with `</dev/null` stdin (src/remote/attach.rs#L417-L425) through `connect_saved_ssh(profile_id, target, session)` (src/remote/saved.rs#L15-L30).
- **SSH is OpenSSH, configured by flags, never reimplemented.** Non-interactive connections get `BatchMode=yes`, `NumberOfPasswordPrompts=0`, `StrictHostKeyChecking=yes`, `ConnectTimeout=10`, `ConnectionAttempts=1`, `ServerAliveInterval=15`, `ServerAliveCountMax=4` (src/remote/attach.rs#L1210-L1225); every connection asks for compression `-C` ("multiplexed bridges inherit the master's transport") and, when `remote.manage_ssh_config` is on, a herdr-scoped `-F` config and `-S` ControlPath with `ControlMaster=auto`, `ControlPersist=600` — with the explicit rule "never stop or unlink a master belonging to the user's SSH setup" (src/remote/attach.rs#L1227-L1246). The docs frame this as sharing one *approved* OpenSSH connection across `machine add`, the sidebar and CLI commands; `herdr machine reconnect` completes native auth interactively, and clicking `! auth` / `! error` "never opens a credential popup" (connecting-machines.mdx#L12-L16).
- **Bridge liveness is measured on a suspend-aware clock.** The remote bridge records activity and expires after `IDLE_TIMEOUT = 60 s` of no traffic in either direction; "This clock includes suspend, so short maintenance wakes can reap old bridges" (src/platform/remote_bridge.rs#L8-L19). Cleanup leaves the remote server and panes running (connecting-machines.mdx#L94).

### Version negotiation, install and upgrade of the remote binary

- **Compatibility is negotiated, not version-matched.** `remote_server_restart_reason` returns a reason only when the server's endpoint-protocol *generation* differs, a saved-machine endpoint lacks `surface_interest` or `health_check` capabilities, or the server is not a detached daemon; otherwise `KeepRunning` (src/remote/restart_policy.rs#L16-L37). A restart plan is `KeepRunning | LiveHandoff | StopRequired(reason)`, and live handoff is used only when both offered and enabled (src/remote/restart_policy.rs#L39-L62). Docs: "The client and server negotiate compatibility rather than requiring identical versions … Other missing server methods disable only their corresponding actions" (connecting-machines.mdx#L124); "A version difference alone does not replace or restart a running remote server"; default answer to stopping is No (persistence-remote.mdx#L87).
- **Discovery before install.** `find_installed_remote_herdr` probes the host; on Windows a PowerShell snippet also inspects running `herdr.exe` processes owned by the SSH user and the `packages\standalone\current` junction, emitting base64-marked paths (src/remote/attach.rs#L1649). The remote platform is read from `uname` (src/remote/attach.rs#L290) and mapped to an asset key.
- **Install = stream the binary over the same ssh.** The prepare script prints `tmp\0dest\0` under `$HOME`, the binary is piped through `tee <tmp>`, then `chmod 755 && mv tmp dest` commits atomically (src/remote/attach.rs#L1096-L1141). Source is the local binary when platforms match, else the release asset for the *current client version* from `https://herdr.dev/latest.json` / `preview.json`, or `HERDR_REMOTE_BINARY` (src/remote/attach.rs#L33-L35; persistence-remote.mdx#L95-L97). The Windows path runs the published installer with `-LocalPackageSha256` (src/remote/attach.rs#L1167-L1172).
- **Interactive vs background asymmetry is a rule:** "Background connections never answer prompts or install, update, restart, or hand off a server" (connecting-machines.mdx#L96); setup runs only in an interactive terminal "so Herdr can ask before installing or replacing anything" (connecting-machines.mdx#L30-L38, #L52).

### Combined agent list and what streams

- Each machine is a `ClientEndpointId::Local | Ssh(ProfileId)` (src/client/endpoint.rs#L74-L77); the sidebar lists Local plus every enabled saved machine; agent rows get a `machine` token only when more than one machine is present (configuration.mdx#L386).
- **Only the selected machine streams screens.** "The selected machine receives your pane input and terminal size, and supplies the visible terminal content and graphics. Other connected machines keep updating their workspace information, agent states, and notifications without streaming their pane screens" (connecting-machines.mdx#L58). The mechanism is a **surface lease**: `set_client_shell_surface_active(client_id, active)` advances a projection-revision floor on every activation so a stale `PaneSurface` from an earlier epoch can be distinguished, discards pending renders when inactive, and releases held inputs (src/server/headless/surface_interest.rs#L4-L61). `surface_interest` is therefore a required server capability for saved machines (src/remote/restart_policy.rs#L27-L28).
- IDs are server-scoped: "Two machines may both contain `w1:p1` or an agent named `reviewer`"; CLI automation uses `herdr --machine <label-or-id> agent list` which goes over the saved profile's ssh directly, "it does not need an open TUI" (connecting-machines.mdx#L116).
- The saved-machine CLI is `herdr machine list | add <host> [--label] [--remote-session] | rename | disable | enable | remove <profile-id>` (cli-reference.mdx#L53-L59), plus `herdr --machine <label-or-id> <command>` to run `workspace`, `worktree`, `tab`, `pane`, `notification`, `agent`, `api snapshot`, `status server` and API-backed plugin commands on that machine over its saved ssh target; "Local installation/configuration commands, plugin installation, session management, and interactive terminal attachment are not forwarded" (cli-reference.mdx#L76-L85).
- Keyboard: `prefix+w` highlights workspaces across machines in sidebar order, disconnected machines skipped; a highlighted remote workspace needs Enter *before* other shortcuts "to prevent pane, tab, workspace, or custom-command shortcuts from acting on the current machine by mistake" (connecting-machines.mdx#L62-L64).

### Reconnect and restart policy

- Supervisor backoff: 500 ms initial, capped at 120 s (30 s for Local), and the backoff resets only after a connection has been healthy for `STABLE_CONNECTION_PERIOD = 60 s` — "brief successful connections do not reset it" (src/client/endpoint/supervisor.rs#L11-L14; tests at #L457, #L484; connecting-machines.mdx#L90).
- Application-level health: heartbeat every 5 s, timeout 10 s (src/client/endpoint/health.rs#L3-L4), so "a broken connection does not stay Online indefinitely"; Local uses native closure detection instead (connecting-machines.mdx#L90).
- Failure UX: lost connection ⇒ last workspace and agent state stay visible **dimmed** as cached data, input into cached panes disabled "until a fresh connection and matching screen arrive", and "Reconnecting never takes selection away from the machine you are using" (connecting-machines.mdx#L68). Attention badges `! auth` / `! error` with the latest error and the CLI command to run (connecting-machines.mdx#L91). A notification whose endpoint is offline shows "<label> is unavailable" instead of jumping (src/client/shell/notification_policy.rs#L69-L76). "A stalled machine cannot hold up another machine's input"; Local opens immediately without waiting for SSH (connecting-machines.mdx#L66).

Failure modes and what the user sees (all from connecting-machines.mdx):

| Situation | UI | Recovery |
|---|---|---|
| Network blip / sleep / ssh failure | **Reconnecting**, cached state dimmed, input disabled | automatic retry, backoff to 2 min, fast retry only after 1 min healthy (#L90) |
| Auth failed | `! auth` badge, click shows the error + `herdr machine reconnect <id>` | interactive native ssh auth; `ssh-add` for passphrase keys (#L91, #L104) |
| Host key unknown/changed | **Attention** | user action in ssh; never auto-accepted (#L16) |
| Server too old (no `surface_interest`/`health_check`) | **Attention** until explicitly updated | `herdr --remote <host>` interactively; "Do not stop a running server merely because its version differs" (#L102, #L124) |
| Saved-machine catalog unreadable | notice, connections unchanged | automatic re-read (#L92) |
| Viewed machine removed/disabled | returns to Local; if Local is down, shows that and retries | (#L86) |
| Quiet healthy link | health checks only; remote bridge closed after 1 min idle, server keeps running | reopened on demand (#L94) |

### Credentials and ssh-agent

- Saved profiles hold "only an opaque ID, label, SSH target, explicit remote session, and enabled state … Authentication stays with OpenSSH" (connecting-machines.mdx#L120); a separate optional cache remembers remote OS and executable path and "never automatically repeats a command that may already have changed remote state" (connecting-machines.mdx#L122).
- Agent forwarding is the user's `ForwardAgent yes`; herdr does not enable it. What herdr adds is a **stable agent address per session**: `src/platform/ssh_agent.rs` is "Session-local indirection for SSH agents whose sockets belong to an attachment" — a registry of candidate agent sockets fingerprinted by device/inode/mode/ctime, checked every 1 s, retried every 5 s when unavailable, so panes keep a working `SSH_AUTH_SOCK` across reconnects, with "a working inherited agent or earlier connection keeps priority over later clients" (src/platform/ssh_agent.rs#L1-L30; connecting-machines.mdx#L106-L108). The client side registers its agent with the remote server through `Registration::start()` and retries while the API is not yet ready or during handoff (src/remote/ssh_agent.rs#L11-L24, #L46).

### Security model

- Remote → local: the remote server can only send protocol messages; clipboard writes are forwarded to the foreground client only (A5), and the one bridged local feature is image paste, done by copying the image to a remote temp file and pasting its path (persistence-remote.mdx#L55). The local client uses *its* theme, keybindings and sidebar config; "Herdr does not copy local command plugins, configuration, executables, or secrets onto SSH hosts. Missing remote commands fail visibly" (connecting-machines.mdx#L112).
- Local → remote: the client can install/replace the server only interactively with confirmation; `StrictHostKeyChecking=yes` in background mode, unknown or changed host key stays Attention (connecting-machines.mdx#L16). The local bridge socket is 0600 (src/remote/attach.rs#L26).
- Tests simulate ssh with a shell script on `PATH` that prints a login banner, the `herdr-remote-output-ready:1` marker, `uname` output and later fails with exit 255, gated by approval files (tests/remote_attach.rs#L80-L104; marker constant src/remote/attach.rs#L36); `tests/machine_api.rs` counts `ssh_calls()` to assert that cached metadata skips discovery (tests/machine_api.rs#L187, #L307-L337).

### Weaknesses

- `src/remote/attach.rs` is 5.7 K lines of shell-string construction for POSIX and PowerShell (install, discovery, bridge, handoff) — the Windows discovery one-liner alone is ~1 KB (src/remote/attach.rs#L1649). Fragile by construction; the test strategy (fake ssh scripts) compensates but cannot cover real OpenSSH behaviour.
- Managed SSH recovery is Linux/macOS only ("Local Windows recovery is not supported yet", connecting-machines.mdx#L16); direct terminal attach is not on native Windows (persistence-remote.mdx#L115).
- A machine profile targets one remote *session*; several sessions on one host are several profiles (connecting-machines.mdx#L40).
- Compression is forced with `-C`, which ignores the user's `Compression no` (connecting-machines.mdx#L10) — a reasonable default for screen deltas but a global override.
- Everything is pull from the client: there is no fleet view when no client is open, and a phone has to SSH in; agent states from other machines are refreshed only while a client is connected.

### Borrow candidates for theuth

1. **Capability-negotiated compatibility with explicit restart reasons** (`EndpointProtocol | SurfaceInterest | HealthCheck | DaemonDetach` → `KeepRunning | LiveHandoff | StopRequired`) instead of version equality. **S**, lands in crates/mesh + crates/update (39): engines on build-A/B and sandboxes can stay up across most theuth releases, and the UI can say *why* an engine needs a restart.
2. **Stream-the-binary-over-the-existing-ssh install with tmp/`tee`/`mv` commit and asset fallback for mismatched platforms**. **S–M**, lands in crates/nodes + crates/sandbox (bootstrapping an engine on a fresh host or VM without a package manager).
3. **Surface lease / projection floor: only the viewed engine streams screens, every machine streams states.** **M**, lands in crates/rpc + crates/mesh + desktop (129 EVENT-FOCUS already moved in this direction; this is the explicit epoch counter that makes stale frames detectable).
4. **Backoff that resets only after 60 s healthy + 5 s/10 s heartbeat + suspend-aware idle clock.** **S**, lands in crates/mesh engine links and the companion's engine connection (the "engine unreachable" toast on Windows was exactly a missing grace rule).
5. **Cached-but-dimmed remote state with input disabled until a fresh frame; reconnect never steals selection.** **S**, lands in desktop sidebar for engines/devices.
6. **Stable per-session `SSH_AUTH_SOCK` indirection** so PTY jobs on a remote engine keep git/ssh auth across reconnects. **M**, lands in crates/runtime + crates/nodes; only if theuth starts running jobs on remote engines via ssh rather than its own mesh.
7. **Fake-ssh-on-PATH test harness with marker lines and approval files** for anything that shells out to ssh. **S**, lands in crates/nodes tests.

## A7 Terminal engine & multi-client rendering

**Pointer to A1:** the screen-manifest detector reads the ghostty screen text; its arbitration is the sibling's. Here: the engine, the wire, and what reaches a client.

### What `crates/ghostty-vt` wraps and how it builds

- **It is a Rust binding to a vendored `libghostty-vt`** — "Herdr's binding to its vendored libghostty-vt" (crates/ghostty-vt/Cargo.toml#L1-L7). The vendored tree is Ghostty's Zig VT library at upstream commit `44f2a44df`, dist archive `libghostty-vt-1.3.2-HEAD-+44f2a44df.tar.gz` (vendor/libghostty-vt.vendor.json#L1-L5); 757 `.zig` files, ~22 MB on disk. There is **no vt100/alacritty_terminal crate**: Cargo pins `portable-pty = "=0.9.0"` and patches it to `vendor/portable-pty` (Cargo.toml#L39, #L57), and the workspace's only member crate is `ghostty-vt` (Cargo.toml#L24-L28).
- **Build = `zig build` from `build.rs`, requires Zig 0.16.0.** `build.rs` maps the Rust target triple to a Zig target (10 triples incl. iOS and Windows MSVC), runs `zig build --prefix <OUT_DIR>/zig-out -Demit-lib-vt -Doptimize=ReleaseFast -Dsimd=true -Dtarget=… -Dversion-string=…`, with env knobs `LIBGHOSTTY_VT_OPTIMIZE`, `LIBGHOSTTY_VT_SIMD`, `LIBGHOSTTY_VT_ZIG_SYSTEM_DIR`, `ZIG`, `LIBGHOSTTY_VT_WINDOWS_LIBC`; the panic text says "Building the vendored libghostty-vt requires Zig 0.16.0" (crates/ghostty-vt/build.rs#L6-L20, #L52-L83, #L97-L116). It installs into `OUT_DIR` "so concurrent builds for different targets from one checkout never link each other's archive" and on Apple copies only the static archive into a directory of its own because "Apple's linker prefers the sibling dylib" (crates/ghostty-vt/build.rs#L67-L70, #L118-L130). macOS builds statically link it to keep the single-binary install (CHANGELOG.md#L1136).
- **FFI surface.** `bindings.rs` (5,376 lines, 200 `pub fn ghostty_*` externs) plus a hand-written safe layer in `lib.rs` (5,079 lines, 112 `pub fn`): `screen_text_rows`, `read_text_viewport`/`read_ansi_viewport`/`read_text_screen`/`read_ansi_screen`, `viewport_link_regions`, `scroll_viewport_*`, `effective_foreground_color`, `kitty_graphics_generation`, `kitty_image_placements…` (crates/ghostty-vt/src/lib.rs#L1291-L1319, #L1561-L1610, #L1715-L1737, #L1846-L1897). Integration tests use the raw C API too: the test observer in `tests/support/terminal_screen.rs` builds a `GhosttyTerminal` + `GhosttyFormatter` and formats PLAIN text from raw bytes (tests/support/terminal_screen.rs#L3-L30) — i.e. tests do not need a second emulator to assert on screens.
What the safe layer exposes (a sample of the 112 `pub fn`s, crates/ghostty-vt/src/lib.rs):

- screen/viewport reads: `screen_cell`, `screen_text_rows`, `for_each_screen_row_cell`, `screen_text_rows_range` (#L1284-L1318); `read_text_viewport`, `read_ansi_viewport`, `read_text_screen`, `read_ansi_screen` (#L1561-L1609) — the two "ansi" variants are what direct attach and `agent read --format ansi` use.
- links: `viewport_hyperlink_uri`, `viewport_link_target`, `viewport_link_regions` (#L1364-L1442).
- scrolling/geometry: `scroll_viewport_bottom`, `scroll_viewport_delta`, `scroll_viewport_row`, `cols`, `rows`, `cursor_y`, `width_px`, `height_px` (#L1715-L1772).
- colours for host-theme pass-through: `effective_foreground_color`, `effective_cursor_color` (#L1760-L1764).
- kitty graphics: `kitty_graphics_generation`, `kitty_graphics_may_have_placements`, `kitty_image_fingerprints`, `kitty_image_placements`, `kitty_image_placements_with_data_filter` (#L1846-L1897) — generation counters and fingerprints so the server can skip re-sending unchanged images.
- a separate scrollbar/formatter object with `new`/`update(terminal)` (#L2889-L2903).
- **Why Ghostty (from the CHANGELOG, no git history):** 0.4.0 (2026-04-05) "Herdr now uses a Ghostty-backed terminal engine as its pane runtime. The legacy vt100 pane backend has been removed, making Ghostty the single terminal backend going forward" (CHANGELOG.md#L1123-L1126). The follow-ups show what they bought: byte-based scrollback matching Ghostty (`advanced.scrollback_limit_bytes`, CHANGELOG.md#L1107), SIMD builds on Linux (#L1108), "Ghostty's native selection extraction, which preserves wrapped text and wide characters more accurately" (#L1114), a persistent per-pane key encoder after a typing-latency regression (#L1111), host-theme default background pass-through (#L1039), later a reflow abort fixed when "pre-resize cursor row is past the new height" (#L601) and a frame-streaming crash fixed by a vendored update (#L653). The cost shows too: a Nix flake needing Darwin SDK helpers for the Zig build (#L657), and a C++ runtime link workaround for static Linux builds that was later removed (#L920).
- **Patches are governed.** AGENTS.md requires every local patch to be listed in `vendor/libghostty-vt.patches.md` with "why the patch exists, the Herdr issue, upstream PR/discussion, vendored base commit, touched files, verification, and the exact removal condition", and `just check` verifies patch files are indexed and reverse-apply cleanly (AGENTS.md#L207-L215). Five libghostty patches are active: 0002 a typed query for modifyOtherKeys mode 2 "without formatting or allocation", 0004 hosted Wuffs builds for Windows cross-compilation, 0005 bounded word selection for wrapped links, 0006 clear-screen preserving the cursor line, 0007 experimental encoded-PNG retention (vendor/libghostty-vt.patches.md#L7-L50, #L51, #L93, #L136, #L174).

### The vendored portable-pty 0.9.0 patches (all three are Windows-only)

Same governance file shape (vendor/portable-pty.patches.md#L1-L5):
1. **0001 control ConPTY loading** — upstream "intentionally probes a bare `conpty.dll` through the DLL search path. Herdr must never load another application's DLL from `PATH`"; herdr ships a pinned Microsoft ConPTY runtime, verifies DLL and OpenConsole hashes, rejects reparse points, loads by absolute path with search limited to that directory + System32, falls back to kernel32's ConPTY exports, `HERDR_WINDOWS_CONPTY=system` to bypass (vendor/portable-pty.patches.md#L7-L52; issues #761, #1533; microsoft/terminal#17452).
2. **0002 Windows raw command tails** — launch `cmd.exe /d /c <user text>` unquoted because argv + ArgvQuote changes how cmd parses (vendor/portable-pty.patches.md#L54-L86; issue #1041).
3. **0003 reject malformed Windows environments** — registry `REG_MULTI_SZ`/empty-name entries produce an env block that makes `CreateProcessW` fail with error 87; import only string types and drop entries that cannot form `name=value\0` (vendor/portable-pty.patches.md#L88-L128; issue #3430; wezterm#4364 and a Warp commit as prior art).
Nothing in the patch set touches Unix PTY behaviour; a theuth on portable-pty 0.9 unpatched is only exposed on Windows.

### Server-side headless terminal state

- The server holds every pane's `TerminalRuntime` (ghostty terminal + trackers) and renders *for each client* through a virtual ratatui `TestBackend` — "Virtual rendering helpers for headless client frame streaming" (src/server/render_stream.rs#L1-L12). Per-client state is one of two negotiated encodings: `Semantic { last_surface, surface_revision, surface_reuse, surface_delta, surface_scroll, recompute_pending }` or `TerminalAnsi { blit_encoder, seq, repaint_pending }` (src/server/render_stream.rs#L14-L31; enum `RenderEncoding::SemanticFrame | TerminalAnsi` at src/protocol/wire.rs#L41-L48).
- Pane runtime glue (`src/pane/terminal.rs`, with ghostty key/mouse encoders, kitty-keyboard tracker, OSC trackers for cwd, default colours, XTGETTCAP, DECSCUSR) is 1,063 lines of `src/pane/terminal/*` plus the 13 K `src/pane` tree (src/pane/terminal.rs#L24-L40).

### Wire format and bandwidth tricks

- Binary framing with a u32 LE length prefix, `PROTOCOL_VERSION = 22`, frame cap 2 MB, a 32 MB cap only for explicit graphics frames, 16 MB clipboard-image cap (src/protocol/wire.rs#L1-L34). The *client-shell endpoint contract* is negotiated separately and kept stable across versions: "Variant order is frozen for endpoint generation 1" (src/protocol/wire.rs#L1379-L1381; `ENDPOINT_PROTOCOL_GENERATION` in src/protocol/endpoint.rs, 420 lines).
- Three **optional capability codecs**, each a frozen `endpoint.*.v1` message kind the client advertises:
  - `surface_reuse` — "retains unchanged terminal cells across projections": a frame whose cells are stripped and replaced by `base_surface_revision` (src/protocol/surface_reuse.rs#L1-L25).
  - `surface_delta` — "Optional sparse delivery of a completely recomputed pane surface": `{base_projection_revision, base_surface_revision, rows: patch rows, popup_cells: Patch|Replace}`, ≤ 4096 spans, binary layout frozen with the message kind (src/protocol/surface_delta.rs#L1-L47).
  - `surface_scroll` — "Scrolling output moves every visible row, so a row diff resends the whole pane on each frame. This codec sends each pane's vertical shift once, plus only the rows that still differ after it"; ≤ 64 scroll rects of 10 bytes each, applying a scroll "never clones cell content" (src/protocol/surface_scroll.rs#L1-L30).
- **TerminalAnsi** is for direct terminal attach and thin bridges: `BlitEncoder` diffs against the last frame, writes only changed cells wrapped in synchronized output (`CSI ?2026`), hides the cursor during the paint, repeats the final cursor anchor for IMEs except on Windows Terminal (src/protocol/render_ansi.rs#L1-L27). Observers get newline-delimited JSON `terminal.frame` records with base64 ANSI (persistence-remote.mdx#L139-L149).
- SSH links add `-C` compression on top (A6).

### Alt-screen reads

`alt_screen_read.rs` is a server-side state machine that harvests history from a full-screen app that has no scrollback: phases `SettleInitial → ProbeBottom → RestoreProbe → Harvest → Restore`, with 10 ms quiet windows, a 120 ms step timeout, 15 s overall cap, 5 s cap on restoring the view, and 3 wheel events per step; it snapshots the screen, scrolls upward with synthetic wheel events, merges rows (`UpwardMerge`), and restores the app's view, falling back to a prepared response on failure (src/server/alt_screen_read.rs#L11-L50). This is what makes `pane read --lines N` meaningful inside Claude Code/Codex TUIs.

### Multi-client attach semantics

- Each client views its own workspace/tab; "when clients view the same tab, the last one to focus, select, or interact with it controls that tab's pane sizes" (concepts.mdx#L73). Tested end-to-end: `same_tab_geometry_follows_meaningful_client_activity` reads `stty size` inside the pane, `api_pane_output_is_fanned_out_as_pane_surface_updates`, `crashed_client_shell_does_not_affect_survivor`, `rapid_client_shell_connect_disconnect_remains_healthy` (tests/multi_client.rs#L217-L225, #L270, #L303, #L337, #L370).
- Direct attach: "Only one writable direct attach client owns input and resize for a terminal. Use `--takeover` to replace an existing owner"; read-only observers are unlimited (persistence-remote.mdx#L133-L149).
- Focus reconciliation across shells lives in `client_views.rs` (`ShellFocusTarget {tab_id, workspace_index, pane_id}`, `reconcile_client_shell_locations`, src/server/headless/client_views.rs#L5-L10, #L128).

### Kitty graphics

Standard kitty protocol from pane output; overlays *crop* images rather than hide them; on local Unix Ghostty clients image data can travel as **temporary files** after a "small file-consumption probe", and native RGBA images go server→client "as paths to Herdr-owned snapshots rather than pixel bytes"; Linux can use copy-on-write snapshots into a private `/var/tmp` store, limited to 16 MiB each / 64 MiB total; "Remote server paths are never forwarded to the local terminal"; PNGs are fully decoded and validated server-side (configuration.mdx#L526-L549; code in `src/kitty_graphics*` 2.5 K lines and `crates/ghostty-vt/src/pane_graphics_files.rs` 1,041 lines).

### Performance instrumentation (no published numbers found)

`render_scale_benchmark.rs` measures stage median/p95/max at 120×40 for 1, 15 and 50 panes (src/server/render_scale_benchmark.rs#L14-L24); `HERDR_RENDER_PROF` turns on a windowed profiler (src/render_prof.rs#L1-L9). CHANGELOG describes outcomes qualitatively ("Images render faster", CHANGELOG.md#L29) — I found no absolute latency or bandwidth figures in docs or CHANGELOG.

### Weaknesses

- A Zig 0.16 toolchain in the build, a 22 MB vendored Zig tree and a 5 K-line hand-written FFI layer; the Nix and static-Linux breakages in the CHANGELOG are the price. For theuth this is the single most expensive thing in herdr to copy.
- Three overlapping capability codecs (reuse/delta/scroll) plus two base encodings — each frozen — is a lot of protocol surface for a TUI; the headless server does per-client virtual rendering, so CPU scales with clients × panes (hence the scale benchmark).
- The alt-screen read is heuristic (quiet windows, synthetic wheel events) and bounded by 15 s; on an app that repaints continuously it returns the fallback.
- Graphics file transport is local-Unix-Ghostty-only; everything else stays inline base64 with a 32 MB frame cap.

### Borrow candidates for theuth

1. **Patch governance file for vendored crates** (`vendor/<crate>.patches.md` + `vendor/patches/<crate>/NNNN-*.patch`, each with issue, upstream link, base, files, verification and *removal condition*, checked by a maintenance test). **S**, lands in the repo root / justfile; relevant the day theuth forks portable-pty or vt100.
2. **Windows ConPTY hardening** — never load a bare `conpty.dll` from PATH; hash-verified bundled OpenConsole or kernel32 fallback. **M**, lands in crates/runtime (PTY jobs on build-B / Windows engines). Also 0003's env-block sanitiser (**S**) if theuth ever sees `CreateProcessW` error 87.
3. **Scroll-aware surface patches** (send a vertical shift once + residual rows) for theuth's PTY `screen()` streaming to the desktop over the mesh. **M**, lands in crates/runtime + crates/rpc; only worth it when PTY screens are streamed live to remote clients.
4. **Alt-screen harvest state machine** so `screen()` can return more than the visible rows of a TUI harness (omp/claude in a PTY). **M**, lands in crates/runtime; useful for the orchestrator's "what happened in that PTY" reads.
5. **A raw-FFI test observer** (`tests/support/terminal_screen.rs`) that formats PTY bytes to plain text in tests without a second emulator. **S**, lands in crates/runtime tests using vt100 the same way.
6. **Do not borrow:** the Ghostty swap itself (see Do-not-borrow).

## A8 Workspaces, worktrees & plugins

### Worktree lifecycle

- **A worktree is a workspace with git provenance, grouped under its parent.** "Worktrees are normal Herdr workspaces with Git checkout provenance. `worktree create` creates a Git worktree checkout, opens it as a workspace, and groups it with the parent repo workspace" (cli-reference.mdx#L182). API: `worktree.list | create | open | remove` (socket-api.mdx#L104, #L342); `worktree.create` returns the new `workspace`, `tab`, `root_pane` and `worktree` records in one reply, `worktree.open` returns the already-open workspace instead of duplicating it (socket-api.mdx#L342-L359). CLI flags: `--branch`, `--base REF`, `--path`, `--label`, `--focus/--no-focus`, `--trust-repository` (cli-reference.mdx#L176-L179).
```text
herdr worktree list   [--workspace ID | --cwd PATH] [--trust-repository]
herdr worktree create [--workspace ID | --cwd PATH] [--branch NAME] [--base REF] [--path PATH] [--label TEXT] [--focus|--no-focus] [--trust-repository]
herdr worktree open   [--workspace ID | --cwd PATH] (--path PATH | --branch NAME) [--label TEXT] [--focus|--no-focus] [--trust-repository]
herdr worktree remove --workspace ID [--force] [--trust-repository]
```
(cli-reference.mdx#L176-L179; the same four verbs over `--machine <id>` run on a remote machine with absolute or `~/` paths, cli-reference.mdx#L76-L85.)
- **Naming.** Checkout path = `<worktrees.directory>/<repo>/<branch-slug>`, default root `~/.herdr/worktrees` (configuration.mdx#L96-L105; `default_checkout_path` src/worktree.rs#L171-L173). `branch_to_path_slug` lowercases, collapses every non-alphanumeric run to one `-`, trims, and falls back to `worktree` (src/worktree.rs#L34-L54). A generated branch name is `worktree/<adjective>-<noun>-<hex4>` from 8×8 word lists and a seed (src/worktree.rs#L21-L32) — memorable, namespaced under `worktree/`, no collision with user branches.
- **Base branch.** New branch: `git worktree add -b <branch> <path> <base>` (src/worktree.rs#L239-L259); if the branch already exists locally it is checked out instead (`local_branch_exists`, src/worktree.rs#L261-L280); base defaults to `HEAD` (cli-reference.mdx#L182). Git runs with `-C <repo_root>` and, only when the caller passes `--trust-repository`, a request-scoped `-c safe.directory=<root>` (src/worktree.rs#L154-L169) — the trust is per request, not written to global config.
- **Cleanup.** `worktree.remove` runs `git worktree remove [--force] <path>` and **never deletes the branch** (src/worktree.rs#L175-L192; cli-reference.mdx#L184); dirty-checkout refusal is detected from git's own message text ("contains modified or untracked files" + "use --force", or the submodule refusal) and the UI asks again before forcing (src/worktree.rs#L194-L199; configuration.mdx#L111). Forced removal has a recovery path for a *leftover unregistered checkout*: if git says it is not a working tree, the path is removed only when it is no longer in `git worktree list` **and** the leftover directory provably belongs to this repo (src/worktree.rs#L346-L375). Closing the parent closes the Herdr group but "does not delete checkout folders or branches" (configuration.mdx#L109); the API refuses to close a primary workspace with open children unless `close_group: true`, error `workspace_group_close_required` (socket-api.mdx#L366; cli-reference.mdx#L184).
- **Which agent runs where** is implicit: a worktree workspace gets its own tab/root pane with cwd in the checkout; the agent is whatever is launched in that pane. Plugins can hook `on = "worktree.created"` to bootstrap (plugins.mdx#L84-L86), which is how the marketplace example `worktree-bootstrap` works (cli-reference.mdx#L476).
- The worktree API module is large: src/app/api/worktrees.rs 2,783 lines + `deferred.rs` 703 (membership validation waits on the git worker) + `reads.rs` 340.

### Workspace git status discovery and refresh cost

- Discovery reads `.git` **files directly** rather than spawning git for everything: `GitWorktreeInfo {repo_root, git_dir, git_common_dir, is_bare, is_linked_worktree}` (src/workspace/git/discovery.rs#L15-L22), ref files capped at 64 KiB (src/workspace/git/discovery.rs#L4), reftable storage detected (`git_ref_storage_is_reftable`), branch config parsed from `.git/config` into `BranchConfig {remote, merge_ref, fetch_refspecs, remote_urls}` with a **file-stamp dependency cache**: every parsed file is recorded as `(path, (mtime, len), reusable, symlink target)` and `deps_current()` short-circuits the refresh when none changed (src/workspace/git/config.rs#L8-L30). Only ahead/behind spawns git: `git rev-list --left-right --count <range>` (src/workspace/git/status.rs#L323-L326), and `rev-parse --verify` for OIDs (src/workspace/git/discovery.rs#L234).
- Refresh demand is split into `{branch, ahead_behind}` so a sidebar without `git_status` tokens never pays for `rev-list` (src/workspace/git/status.rs#L16-L31). A **single detached worker** drains a shared queue "so stalled I/O cannot block server shutdown"; a stalled checkout keeps its slot and is not retried every tick; spawn failure retries after 30 s (src/app/git_refresh.rs#L58-L70, #L101-L102, #L342).
- Aggregate state: a workspace's rolled-up state and `PaneDetail {state, seen, last_agent_state_change_seq, tokens}` feed the Space rows (src/workspace/aggregate.rs#L9-L18).

### Layouts and custom commands

- `layout.export` returns a portable tab tree; `layout.apply` creates a fresh tab from a declarative tree; `layout.set_split_ratio` adjusts one split; caps `MAX_LAYOUT_PANES = 24`, `MAX_LAYOUT_DEPTH = 16` (src/app/api/layouts.rs#L14-L15; socket-api.mdx#L108, #L197-L218). `src/layout.rs` (1,348 lines) is the split tree itself.
- Custom command keybindings: `type = popup | pane | shell | plugin_action`; popup is a session-modal terminal that "receives all terminal input, including Escape", sized in cells or percent; `shell` runs detached; commands receive `HERDR_SOCKET_PATH`, `HERDR_BIN_PATH`, `HERDR_ACTIVE_{WORKSPACE,TAB,PANE}_ID`, `HERDR_ACTIVE_PANE_CWD` and run from the focused pane's cwd (configuration.mdx#L190-L247).

### Plugin model

- **Argv processes, no SDK.** "There is no separate plugin SDK or restricted command set. The entire Herdr CLI is the plugin API"; plugins call back through `HERDR_BIN_PATH` (portable across Unix sockets and Windows named pipes) or the raw socket (plugins.mdx#L18-L29). "Runtime action registration and native non-terminal plugin UI are not part of plugin v1" — everything is declared in the manifest (plugins.mdx#L31-L33).
- **Manifest** `herdr-plugin.toml`: required `id`, `name`, `version`, `min_herdr_version` (install/link refused when the binary is older), `platforms`, `[[build]]`, `[[startup]]`, `[[actions]]` with `contexts`, `[[events]] on = "<event>"`, `[[panes]]` with `placement`, `[[link_handlers]]` with a Rust regex over clicked URLs; `command` values are argv arrays, "Herdr does not run them through a shell" (plugins.mdx#L54-L121). Ids ≤ 120 chars (src/app/api/plugins/manifest.rs#L8-L9).
- **Lifecycle.** Startup hooks run once after session restore and when a new server takes over in live handoff, "not when a client attaches, config reloads, or a plugin is linked or enabled"; they are "one-shot initialization commands rather than supervised daemons" and a failure does not stop the server (plugins.mdx#L247-L263). Event hooks receive `HERDR_PLUGIN_EVENT_JSON`; every command gets `HERDR_PLUGIN_ID/ROOT/CONFIG_DIR/STATE_DIR/CONTEXT_JSON` (plugins.mdx#L265-L289). Runtime caps: 64 KiB of captured output per command, 32 commands in flight, a 200-entry command log (src/app/api/plugins/runtime.rs#L11-L13).
- **Panes.** Placement `overlay` (default, temporary zoomed overlay that restores focus/zoom), `popup` (session-modal, no pane id, no lifecycle events, not persisted), `split`, `tab`, `zoomed`; after opening, plugin panes are normal panes and ownership follows the pane across tabs/workspaces (plugins.mdx#L299-L338).
- **Permissions:** none. "A plugin is ordinary code that runs on your machine … Herdr … does not review or sandbox plugin code"; the mitigation is an interactive **trust preview** of the manifest and every command (incl. startup hooks) before install, `--yes` to skip, `--ref` to pin (plugins.mdx#L35-L52, #L262).
- **Storage:** "There is no Herdr-managed plugin storage API in v1" — config under `HERDR_PLUGIN_CONFIG_DIR`, state under `HERDR_PLUGIN_STATE_DIR`, never in the managed checkout root (plugins.mdx#L277-L284, #L373-L376).

### Distribution and persistence

- **Marketplace = GitHub topic search.** herdr.dev/plugins indexes public repos tagged `herdr-plugin` whose default branch has a parseable `herdr-plugin.toml`; one card per repo, each manifest a separately installable plugin; refreshed every 30 min and on default-branch head change; forks/archived/malformed excluded; "it is not a reviewed catalog" (marketplace.mdx#L6-L8, #L12-L22, #L39-L54).
- **Install = `git` clone of `owner/repo[/subdir]`** into a Herdr-managed directory, build commands from the manifest, requested ref preserved on `plugin update`, reinstall creates a *new* installation directory while "Running plugins keep their original files; new invocations use the new installation" (cli-reference.mdx#L476). Installations carry an `.in-use` lease file (src/plugin_installations.rs#L9); the registry is `plugins.json` beside the config under a `.plugins.lock` file lock (src/persist/plugin_registry.rs#L8-L17). `plugin.link` records `source {kind: github, owner, repo, subdir, requested_ref, resolved_commit, managed_path, installed_unix_ms}` (socket-api.mdx#L479).
- Over `--machine`, API-backed plugin commands are forwarded but "plugin installation … [is] not forwarded" and link paths must be absolute (cli-reference.mdx#L81-L85).

### Weaknesses

- No sandbox, no permission model, no plugin storage API — trust preview is the whole security story (plugins.mdx#L49-L52). Fine for a single-user terminal, not for anything that runs unattended.
- Startup hooks are one-shot; a plugin that needs a daemon must supervise itself; there is no plugin health view beyond the 200-entry log.
- Marketplace discovery via GitHub topic is unreviewed and squatting-prone; one maintainer-controlled index with a 30-minute lag.
- Worktree UX knows nothing about *what runs* in the worktree: no "agent X owns worktree Y" record, no cleanup policy (stale worktrees accumulate under `~/.herdr/worktrees`), no merge-back flow; the branch is never deleted even on remove.
- Git status still shells out for ahead/behind; many workspaces × many refs on a slow disk serialise behind one worker (by design, to protect shutdown).

### Borrow candidates for theuth

1. **Engine-managed worktree API with provenance** — `worktree.create` returning `{workspace, tab, root_pane, worktree}`, `<root>/<repo>/<branch-slug>` naming, `worktree/<adj>-<noun>-<hex>` generated branches, request-scoped `safe.directory`, remove-never-deletes-branch, dirty-refusal detected and re-asked, leftover-checkout recovery, group-close guard. **M**, lands in crates/tasks + crates/agent (spawn_subagent `cwd` from a managed worktree) + crates/runtime; theuth's waves cut worktrees by hand today and hit the `-b … origin/main` upstream trap and stale-worktree disk pressure.
2. **File-stamp dependency cache for git metadata** (parse `.git/HEAD`, refs, config directly; `deps_current()` before any work; `rev-list` only when a view needs ahead/behind). **S**, lands in crates/desktop sidebar / crates/tasks project rows (branch per worktree without spawning git on every tick).
3. **One detached git worker with a shared queue that never blocks shutdown and never retries a stalled checkout per tick.** **S**, lands in crates/runtime or wherever theuth polls repos.
4. **`layout.export`/`layout.apply` portable split trees with caps** for the TUI/desktop multi-pane PTY views. **S–M**, lands in crates/tui + desktop; low priority.
5. **Manifest-declared event hooks with `min_<host>_version` and an install-time trust preview** for theuth skills/MCP repos (`add_skill_repo`, `add_mcp_repo`). **S**, lands in crates/agent skills/MCP loading: show the user every command a skill repo would run before it is enabled.
6. **GitHub-topic marketplace** for theuth skills (`theuth-skill` topic, index refreshed on head change). **M**, lands in the site + relay; worth it only once skills are a public surface.

## A9 Engineering practice

### The strongest AGENTS.md rules (quoted)

`AGENTS.md` is 330 lines (CLAUDE.md symlinks to it) and is written *for agents*, with a scope gate at the top: maintainer sections apply only to verified maintainers, everyone else gets the "External contributor guardrail" (AGENTS.md#L5, #L96-L100, #L319-L329).

- Architecture principles: "**State is separated from runtime.** `AppState` is pure data, testable without PTYs or async"; "**Render is pure** … Never mutate state during render"; "**Platform code is isolated** … Core modules don't have `#[cfg(target_os)]`"; "**Detection is decoupled.** The detector reads a screen snapshot, never touches the parser or viewport state"; "**Screen detection is evidence-based** … capture the relevant bottom-buffer state with `herdr agent read <pane> --source detection` … encode them as explicit AND/OR gates. Do not match whole-pane incidental text, and do not use the user-visible viewport for agent status because users can scroll it" (AGENTS.md#L28-L36).
- **Multiplicative performance paths:** "Treat work reachable from view computation, rendering, background-pane resizing, PTY parsing, detection, and client frame fanout as multiplicative. Before adding work, identify its frequency and cardinality: per byte, event, or render × panes, tabs, or workspaces × attached clients … When a change adds or widens work in one of these loops, profile fixed geometry with 1 and at least 15 populated panes and report the scaling delta" (AGENTS.md#L38-L57). "Prefer deterministic operation or architecture tests to wall-clock CI limits" (AGENTS.md#L59).
- **Runtime/client boundary guardrail:** classify every new field as "Shared runtime/session fact … server state … JSON API/event path" or "TUI presentation state"; "Use neutral server/API names, not UI-surface names like sidebar, row, card, or widget" (AGENTS.md#L66-L81).
- **Stable client endpoint contract:** "Named core codecs are immutable … Introduce a new codec name and keep the old codec as a fallback instead"; "A missing optional feature must disable only that action, not reject the connection"; "Frozen endpoint fixtures, bincode digests, wire-tag tests, and `tests/fixtures/endpoint-method-shapes-v1.json` are compatibility contracts. Never update a generation-1 expectation merely to bless a wire change"; "Existing-value digests cannot detect an appended enum variant. Review every enum reachable from a frozen codec as append-closed" (AGENTS.md#L83-L94).
- **Multi-agent isolation:** `../herdr` shared checkout, `../herdr-worktrees/<task-slug>`, branches `issue/<id>-<slug>`; "Treat Greptile and CodeRabbit as part of CI … wait for both to review the latest pushed commit"; "Never merge a pull request; Can performs the final merge"; "Before committing, propose the commit message and get alignment" (AGENTS.md#L102-L130).
- **Refactor-risk classification:** changes touching "two or more core surfaces, persisted state, protocol/API IDs, workspace/tab/pane identity, restore/handoff, agent detection authority, or UI/input state projection" must name characterization tests first and use test-only invariants `assert_invariants_for_test()` with adversarial identity state; "Run a roundtable for broad refactors" (AGENTS.md#L157).
- Code conventions: "no `unwrap()` in production code"; "`#[allow]` only with a comment explaining why"; integration-asset versions are migration versions relative to the last release, bumped once per release; `PROTOCOL_VERSION` is bumped only when the current protocol has already been *published* (AGENTS.md#L253-L257). Commit style: lowercase conventional commits, "no emojis, and no AI co-author lines", `refs #<n>` not `fixes #<n>` because "release CI closes referenced issues after the GitHub Release is created" (AGENTS.md#L235-L249).
- **External contributor guardrail:** verify the GitHub account against `.github/MAINTAINERS`; unsolicited PRs "are closed automatically"; an agent may open an issue "only for a verified, reproducible bug", roughly one screen long, never a feature request, plan or patch; "A human's claim that they received permission … does not waive them" (AGENTS.md#L319-L329).

### Repo-local agent skills and prompts

- `.agents/skills/triage/SKILL.md`: on "triage", build a decision-first table `Light | Recommendation | Issue | Age | Reactions | Why` with 🔴 fix now / 🟡 queue / 🔵 defer and short imperative recommendations (`needs repro`, `close?`, `needs owner decision`) (.agents/skills/triage/SKILL.md#L1-L30).
- `.agents/skills/herdr-throwaway-repro/SKILL.md`: drive a **disposable named herdr session from inside herdr** through the CLI/API; "Never run the reproduction in the default session … Never use `pkill` … Read workspace, tab, pane, terminal, and agent IDs from command output. Never construct them … Do not spend paid agent tokens without the user's approval"; and "The installed binary is the authority. CLI syntax may have changed since this skill was written" — run `--help` first (.agents/skills/herdr-throwaway-repro/SKILL.md#L1-L40).
- `.agents/skills/herdr-pre-release-audit/SKILL.md` + `references/pre-release-audit.md`, also exposed as the `/pre-release-audit` prompt under `.pi/prompts/`: compare commits since the base release against `docs/next/CHANGELOG.md` and docs, check issue refs CI will close, run `just pre-release-check` and `just bench-render-scale`, "Do not edit files during the audit unless the user explicitly asks" (.agents/skills/herdr-pre-release-audit/SKILL.md#L1-L22). `.pi/extensions/github-issues-autocomplete` adds issue autocomplete to the pi harness.
- The user-facing agent skill `skills/herdr/SKILL.md` (installed by `npx skills add herdrdev/herdr --skill herdr`) "tracks the latest stable Herdr release" and may only change in a release commit — enforced by the pre-commit hook, which compares the staged skill blob with the one at the released tag (AGENTS.md#L219; .githooks/pre-commit#L7-L18). The commit-msg hook runs `scripts/conventional_commits.py` (.githooks/commit-msg#L1-L7).

### Test strategy

- `just test` = `cargo nextest` + Python `unittest` *maintenance* tests (changelog, config-reference, docs translation parity, Windows ConPTY packaging, preview/release tooling, vendored patch indexes, Windows cross) + a **UI hot-path architecture test** + integration-assets + docs-contract tests (justfile#L7-L31). Architecture boundaries are enforced by a Python test, not by review.
- Integration tests spawn the real binary: `tests/support/mod.rs` isolates processes, tracks spawned server PIDs and runtime dirs for cleanup, and hand-encodes the varint/bincode handshake so a test can be a raw client (`client_handshake`, `client_shell_handshake`, `read_server_message`, `wait_for_message_variant`) (tests/support/mod.rs#L29-L127, #L292-L365, #L454-L485). Terminals are asserted through libghostty itself (tests/support/terminal_screen.rs#L3-L30, A7).
- Suites: `server_headless.rs` (23 tests), `live_handoff.rs` (45 — excluded on macOS CI), `client_mode.rs` (48), `multi_client.rs` (18), `detach_reattach.rs`, `session_restore.rs`, `host_shutdown.rs`, `broken_pipe.rs`, `windows_server_security.rs`, plus `remote_attach`/`machine_api`/`machine_setup`/`cross_area` with fake ssh (A6); `tests/cli/*` covers agent options/transport/wait, hooks, plugins, protocol. Fixtures: frozen endpoint `hello/welcome/snapshot/method-shapes-v1.json`, a `keyboard_protocol_corpus.tsv`, per-OS terminal variant tables, `plugin-smoke`, `session` (tests/fixtures/).
- CI (`.github/workflows/ci.yml`): commit-subject and PR-title validation, a 3-OS matrix (ubuntu all tests, macos `not binary(live_handoff)`, windows `just check` + a ConPTY pane smoke) with a 15-minute timeout, cached Zig compiler and Zig build cache, incremental compiler state restore, a dedicated "scalar libghostty with runtime safety" nextest run, and a separate `windows-2022` build job (.github/workflows/ci.yml#L41-L45, #L50-L66, #L100-L107, #L171-L197, #L222). Other workflows: `pr-gate.yml`, `preview.yml`, `release.yml`, `distribution.yml`, `nix.yml`, `windows-arm64.yml`, `label-next-release-issues.yml`, `website-deploy.yml`.
The 24 recipes, by line (justfile#L7-L261): `test` 7, `maintenance-test` 15, `test-windows-input` 21, `test-one` 25, `ui-hot-path-architecture-test` 29, `lint` 34/40 (unix/windows), `ci` 44, `ci-tests` 48, `setup-windows-cross` 56, `windows-lint` 61, `check` 66/72, `install-hooks` 76, `build` 84/88, `bench-render-scale` 92, `bench-terminal-targets` 96, `bench-process-inspection` 101, `bench-bsp-layout` 105, `bench-retained-graphics` 109, `bench-api-fairness` 113, `bench-release-smoke` 117, `docs-contract-test` 122, `integration-assets-test` 126, `libghostty-bindings` 132, `build-libghostty-vt` 136, `release-docs-check` 140, `pre-release-check` 178, `preview` 187, `release-prepare` 198, `release-publish` 227, `release` 256, `default-config` 261. Seven of them are benchmarks, four are release steps, and three are contract tests — the shape of a repo whose risk is in fan-out performance and wire compatibility rather than in unit logic.
- Pinned toolchain `1.96.1` with clippy+rustfmt (rust-toolchain.toml#L1-L3); `clippy.toml` sets `too-many-arguments-threshold = 11` (clippy.toml#L1).

### Release process and channels

- "one main branch and two update channels. Normal previews select a commit from `master`. Stable promotes a published preview, never the latest `master`. There is no long-lived release or preview branch" (AGENTS.md#L266). `just preview <ref>` creates a `preview-<date>-<sha>` tag; stable starts "in an isolated checkout at the selected published preview tag", then `just release 0.x.y preview-<build-id>` with required `Preview` and `Previous-Stable` trailers; "Only the Herdr package version … changelogs, staged READMEs, staged website prose, product announcement, and stable skill may differ from the preview. Code, dependencies, API schemas, build configuration, and other files must match" (AGENTS.md#L284, #L292-L301). Tags are admin-only via a `release-tags` ruleset; both publishing workflows check the actor's admin permission (AGENTS.md#L286).
- Release CI "closes issues using the recorded previous stable boundary, snapshots the tagged docs, and updates `distribution/latest.json`. It applies only the release-preparation diff back to master with a three-way merge" (AGENTS.md#L303). Five assets, and the Windows zip must carry the app-local ConPTY runtime (AGENTS.md#L307-L315).
- `distribution/latest.json` is the update manifest: `{version, protocol, endpoint_generation, notes, assets{5}, sha256{5}, releases{…}}` (distribution/latest.json#L1-L25); the updater reads `https://herdr.dev/latest.json` / `preview.json`, with a Homebrew formula API check and rejection of `channel set preview` on package-manager installs (src/update.rs#L25-L27, #L1939-L1960). Users switch with `herdr channel set preview && herdr update` (AGENTS.md#L270-L282).
- Cadence from CHANGELOG: 59 `## [x.y.z]` entries from 0.1.0 (2026-03-27) to 0.9.3 (2026-09-29) — ~2.3 releases per week over six months; 0.9.x shows the hotfix pattern (0.9.3 the same day as 0.9.2) (CHANGELOG.md#L5-L13, #L1246-L1277).

### Versioned docs and changelog conventions

- Three doc trees: `docs/next/` (committed drafts, "never production website input"), `docs/preview/` (CI-owned snapshot, committed atomically with `distribution/preview.json`, "never edit it manually"), `docs/versions/<v>/` (seeded by release CI from the tag, maintainers may correct afterwards without a release) with `docs/versions/manifest.json` selecting `current` (AGENTS.md#L221-L227; docs/versions/manifest.json#L1-L12 — 25 versions 0.5.11 … 0.9.3).
- "Normal feature and fix work must not edit `docs/next/CHANGELOG.md`; this keeps long-lived branches from conflicting over one shared release file"; the audit "human-write[s] and curate[s] the user-facing entries … generated commit lists are source material, not final release prose"; no entries for website/docs/CI/maintenance changes (AGENTS.md#L229). Preview notes are only a build id and compare link (AGENTS.md#L288). Translation parity (ja, zh-cn) is a maintenance test (justfile#L17).

### Remote-updated detection catalog

`distribution/agent-detection/*.toml` (one per agent: amp, antigravity, claude, cline, codex, cursor, devin, droid, gemini, github-copilot, …) is published at `https://herdr.dev/agent-detection/index.toml`; the client fetches ≤ 256 KiB, keyed by `MANIFEST_ENGINE_VERSION = 3`, overridable with `HERDR_AGENT_DETECTION_MANIFEST_CATALOG_URL` (src/detect/manifest_update.rs#L15-L18). Local overrides live at `~/.config/herdr/agent-detection/<agent>.toml` and reload with `herdr server reload-agent-manifests`; AGENTS.md insists the bundled manifest stays the source of truth and a pre-existing override is never overwritten without alignment (AGENTS.md#L197-L205; agents.mdx#L87). Detection fixes therefore ship between releases.

### Weaknesses

- The process is tuned for one owner ("Can performs the final merge", a Windows VM with a hard-coded account) and heavy bot review; it does not scale to a second maintainer without editing the file.
- CI runs `live_handoff` only on Linux; macOS and Windows are partial. The 15-minute job timeout plus a Zig build means caches are load-bearing.
- Python `unittest` + Bun tests + nextest = three test runners for one repo.
- 59 releases in six months with human-curated changelogs is a large ongoing cost; AGENTS.md is 30 KB of rules that every agent call must read.

### Borrow candidates for theuth

1. **"Multiplicative paths" rule + a scale benchmark recipe (1 vs 15 panes/sessions, report the delta)** for anything on the event-fanout path (129 showed theuth's desktop is exactly such a path). **S**, lands in AGENTS/prompt.md + a `just bench-*` recipe in crates/rpc or crates/desktop.
2. **Append-closed frozen codecs with fixture digests and "add a method/capability, never change one"** for theuth's RPC `Api` + ts-rs bindings across engine/desktop/companion versions. **M**, lands in crates/rpc (bindings drift already bites worktrees).
3. **Repo-local throwaway-repro skill**: disposable named engine/data-dir driven only through the CLI/RPC, with the "installed binary is the authority — run --help first" rule and ID-from-output-never-constructed rule. **S**, lands in skills/ (theuth already has a scratch-engine recipe in knowledge; make it a skill).
4. **Refactor-risk classification + adversarial-state invariant helpers** (`assert_invariants_for_test`, `test_with_adversarial_identity_state`). **S–M**, lands in crates/agent + crates/tasks tests around session/task identity.
5. **Preview→stable promotion where only version/notes may differ from the preview, enforced by a diff check in CI**, and `refs #n` instead of `fixes #n` so release CI closes issues at publish time. **M**, lands in crates/update (39) + release scripts.
6. **Architecture-boundary tests in a scripting language** (ui-hot-path test, patch-index test, docs-contract test) rather than review memory. **S**, lands in justfile/scripts.
7. **Agent-facing skill pinned to the released CLI and gated by a pre-commit hook** so the public `theuth` skill never describes unreleased behaviour. **S**, lands in .githooks + skills/.

### Borrow index across A5–A9 (one line each; sizes S ≤ 1 day, M 2–4 days, L a wave)

| # | Technique | Size | theuth seam | Why now |
|---|---|---|---|---|
| A5-1 | Semantic notification envelope; server decides *whether*, client decides *how*; foreground client only | S–M | attention inbox (104/132), crates/rpc events, companion (130) | the "which one is blocked" complaint is a delivery problem |
| A5-2 | Notify only on →Blocked and background completion; active-view suppression; delay that re-checks state | S | attention filing path | kills flapping ACP children |
| A5-3 | Per-client done/seen bit | S–M | crates/rpc + desktop/companion stores | phone and desktop clear badges independently |
| A5-4 | OSC 9/99 terminal notifications with tmux passthrough | S | crates/tui | phone SSH clients get alerts |
| A5-5 | Declarative sidebar token rows + rules + agent-reported `$tokens` | M | desktop sidebar, crates/tui | subagents can publish `summary`/`model` |
| A5-6 | Goto picker with `b/w/i/d` filters | S | desktop palette / TUI menu | one keystroke to the blocked one |
| A5-7 | Server-rendered outer window title with spinner frames stripped | S | crates/tui, desktop title | glanceable in dock/alt-tab |
| A6-1 | Capability-negotiated compat with explicit restart reasons | S | crates/mesh, crates/update (39) | engines survive most releases |
| A6-2 | Stream-binary-over-ssh install, tmp/`tee`/`mv` commit, asset fallback | S–M | crates/nodes, crates/sandbox | bootstrap a fresh host/VM |
| A6-3 | Surface lease / projection-revision floor: only the viewed engine streams screens | M | crates/rpc, crates/mesh, desktop | continues 129 EVENT-FOCUS |
| A6-4 | Backoff reset only after 60 s healthy; 5/10 s heartbeat; suspend-aware idle clock | S | mesh links, companion engine link | the Windows "unreachable" toast class of bug |
| A6-5 | Cached-but-dimmed remote state; reconnect never steals selection | S | desktop sidebar (engines/devices) | honest UI under flaky links |
| A6-6 | Stable per-session `SSH_AUTH_SOCK` indirection | M | crates/runtime, crates/nodes | only if jobs run over ssh |
| A6-7 | Fake-ssh-on-PATH test harness with marker lines | S | crates/nodes tests | deterministic ssh tests |
| A7-1 | Patch governance file for vendored crates with removal conditions + index test | S | repo root, justfile | the day portable-pty/vt100 is forked |
| A7-2 | ConPTY DLL hardening + Windows env-block sanitiser | M / S | crates/runtime | PTY jobs on Windows engines |
| A7-3 | Scroll-aware surface patches | M | crates/runtime, crates/rpc | only if PTY screens stream live |
| A7-4 | Alt-screen harvest state machine for `screen()` history | M | crates/runtime | read more than the visible rows of a TUI harness |
| A7-5 | Raw-emulator test observer (bytes → plain text) | S | crates/runtime tests | assert on screens without a second emulator |
| A8-1 | Engine-managed worktree API with provenance and safe removal | M | crates/tasks, crates/agent, crates/runtime | waves cut worktrees by hand |
| A8-2 | File-stamp dependency cache for git metadata | S | desktop sidebar, tasks project rows | branch per worktree without spawning git |
| A8-3 | One detached git worker; never block shutdown; no per-tick retry of a stalled repo | S | wherever theuth polls repos | shutdown safety |
| A8-4 | `layout.export/apply` portable split trees with caps | S–M | crates/tui, desktop | low priority |
| A8-5 | Manifest hooks with `min_version` + install-time trust preview | S | skills/MCP repo loading | show commands before enabling a repo |
| A8-6 | GitHub-topic marketplace | M | site + relay | only once skills are public |
| A9-1 | "Multiplicative paths" rule + 1-vs-15 scale benchmark | S | prompt.md, `just bench-*` | the desktop fan-out path |
| A9-2 | Append-closed frozen codecs with fixture digests | M | crates/rpc + ts-rs bindings | version drift across engine/desktop/companion |
| A9-3 | Throwaway-repro skill (disposable engine, CLI only, ids from output) | S | skills/ | already a knowledge recipe |
| A9-4 | Refactor-risk classification + adversarial-state invariants | S–M | crates/agent, crates/tasks tests | identity refactors |
| A9-5 | Preview→stable promotion with diff check; `refs #n` closed at publish | M | crates/update (39), release scripts | release hygiene |
| A9-6 | Architecture-boundary tests in a scripting language | S | justfile/scripts | review memory → test |
| A9-7 | Public agent skill pinned to the released CLI by a pre-commit hook | S | .githooks, skills/ | the `theuth` skill never lies about unreleased behaviour |

## Numbers

All counts from the shallow clone at HEAD 2563803d (v0.9.3); `wc -l` over `*.rs` unless noted.

| Subsystem | Lines | Notes |
|---|---:|---|
| `src/` total | 282,088 | one binary crate |
| `src/client/` | 64,718 | TUI client: shell/, endpoint/, input/; `shell/mouse.rs` 2,402, `shell/state.rs` 1,896 |
| `src/app/` | 38,189 | `actions.rs` 4,800; `api/worktrees.rs` 2,783; `tab_bar_status.rs` 826 |
| `src/server/` (incl. headless) | 29,504 | `headless/` 16,680; `render_stream.rs` 865; `alt_screen_read.rs` 996 |
| `src/platform/` | 14,715 | `macos.rs` 1,415; `windows/notifications.rs` 533 |
| `src/pane/` | 13,007 | ghostty glue, OSC trackers, kitty keyboard |
| `src/terminal/` | 9,099 | runtime registry, history reads |
| `src/protocol/` | 8,900 | `wire.rs` 3,675; `render_ansi.rs` 2,466; `surface_scroll.rs` 720; `surface_delta.rs` 546 |
| `src/remote/` | 6,595 | `attach.rs` 5,703 |
| `src/detect/` | 5,515 | + 22 agent manifests in `distribution/agent-detection/` (23 files incl. `index.toml`) |
| `src/persist/` | 5,139 | `plugin_registry.rs` 451 |
| `src/update.rs` | 4,021 | channels, manifests, Homebrew/Nix guards |
| `src/raw_input.rs` | 3,801 | |
| `src/workspace/` | 3,910 | git discovery/status/config ~3,000 |
| `src/ui/` | 3,721 | sidebar token resolver 587 |
| `src/input/` | 3,313 | |
| `src/kitty_graphics*` | 2,549 | |
| `src/worktree.rs` + `src/layout.rs` | 1,066 + 1,348 | |
| plugins (`plugin_*.rs` + `app/api/plugins/`) | ~890 + api/plugins | runtime caps 64 KiB / 32 in flight / 200 log |
| `crates/ghostty-vt/` | 12,331 | `bindings.rs` 5,376 (200 externs), `lib.rs` 5,079 (112 pub fns), `pane_graphics_files.rs` 1,041 |
| `vendor/libghostty-vt/` | 757 `.zig` files, ~22 MB | 5 local patches |
| `vendor/portable-pty/` | 3,186 | 3 local patches, all Windows |
| `tests/` | 21,536 | `client_mode.rs` 2,257; `live_handoff.rs` 2,120; `support/mod.rs` 950 |
| docs (`docs/next/.../docs/*.mdx`, English) | 4,810 lines / 23 pages | 25 published versions under `docs/versions/` |
| `AGENTS.md` | 330 lines / ~32 KB | |
| `justfile` | 262 lines / 24 recipes | 7 `bench-*` recipes |

Tests: **3,901** `#[test]`/`#[tokio::test]` attributes under `src/` + `crates/`, **255** in `tests/` (23 headless-server, 45 live-handoff, 48 client-mode, 18 multi-client, 16 machine-api, 11 machine-setup, 5 cross-area, 1 remote-attach), plus 13 Python `unittest` maintenance modules and one Bun test file (justfile#L15-L18).

Release cadence: 59 CHANGELOG releases, 0.1.0 on 2026-03-27 → 0.9.3 on 2026-09-29 (~2.3/week); 0.9.x: 0.9.0 09-07, 0.9.1 09-16, 0.9.2 + 0.9.3 (hotfix) 09-29 (CHANGELOG.md#L5-L13, #L123). PR numbers in the CHANGELOG reach #4751 (CHANGELOG.md#L10); the brief's "PR ~#5090" is consistent with the issue numbers cited in the patch files (#3430, #4910 fixture).

Plugins: the marketplace is a GitHub-topic index with no count in the repo; the example repo `ogulcancelik/herdr-plugin-examples` (worktree-bootstrap) is the one cited in docs (cli-reference.mdx#L476). Agents detected: 22 manifests.

Protocol: `PROTOCOL_VERSION = 22`, endpoint generation 1, frame cap 2 MB / 32 MB graphics / 16 MB clipboard image (src/protocol/wire.rs#L20-L31; distribution/latest.json#L2-L4).

Timings baked into code: toast delay ≤ 3600 s; toast durations 8/5/3 s; notification queue 8; ssh `ConnectTimeout 10`, `ServerAliveInterval 15 × 4`, `ControlPersist 600`; bridge idle 60 s; supervisor backoff 0.5 s → 120 s (30 s local), stable after 60 s; heartbeat 5 s / 10 s; alt-screen read 10 ms quiet / 120 ms step / 15 s max; git refresh retry 30 s; status command output ≤ 4 KiB, text ≤ 80 chars.

## Do-not-borrow

- **The Ghostty engine swap.** A Zig 0.16 toolchain in every build, a 22 MB vendored tree, 5 K lines of FFI, five local patches and a documented trail of build breakages (Nix Darwin SDK, static-Linux C++ runtime, SIMD/musl). theuth's PTY jobs render `screen()` text for an orchestrator, not pixels for a human at 60 Hz; vt100 0.16 is adequate until a concrete rendering bug says otherwise. If one does, prefer `alacritty_terminal` (pure Rust) before libghostty.
- **Two render encodings + three frozen capability codecs.** Right-sized for a TUI with remote thin clients; theuth's desktop already has a typed RPC event stream (129). Borrow the *idea* of a scroll-aware patch if PTY screens are ever streamed live, not the codec zoo.
- **A single 282 K-line binary crate with 64 K lines of client.** theuth's workspace-of-crates is the better shape; nothing to import.
- **Plugins as unsandboxed argv processes with the whole CLI as API.** theuth has MCP servers, skills and the Monty runtime with risk-classed tools and approvals; herdr's model is a step backwards in safety for an unattended orchestrator. Borrow only the install-time trust preview and `min_version` gating.
- **External binaries for notification/sound (`terminal-notifier`, `osascript`, `afplay`, `paplay`…).** theuth has a native companion (130/134) and Tauri; use UNUserNotificationCenter/WinRT directly. Borrow the *semantic event*, not the delivery shims.
- **Forcing ssh `-C`, managed `-F` configs and shell-string construction for remote install.** theuth's mesh/nodes already carry their own transport; do not add an ssh-stdio bridge as a second mechanism.
- **GitHub-topic marketplace** until theuth skills are a public surface; and the owner-specific maintainer workflow (Windows VM alias, bot-review gating, "Can merges") — the shape is right, the specifics are herdr's.
- **The phone story** ("SSH in and run the TUI"): theuth's companion/phone surface is already ahead; nothing to take.

## Open questions

1. **Who clears "done" on which surface in theuth?** herdr's per-client seen bit (concepts.mdx#L51) assumes every client is a peer; theuth has desktop + companion + phone + Slack/Telegram + TUI. Decide whether "seen" is per device, per channel or global before borrowing A5-3.
2. **Does theuth want a *delay-and-recheck* on attention items?** herdr re-validates the pane state at delivery (src/app/actions.rs#L1941-L1954). For ACP children the equivalent is "still blocked after N s" — needs the inbox's filing path to support a deferred item, which 132 may or may not have.
3. **Endpoint-generation vs version pinning for theuth's RPC.** AGENTS.md's append-closed codec rule (AGENTS.md#L87-L94) is only worth adopting if engine, desktop and companion are allowed to drift in version. Is that a goal (sandbox engines, build-A/B engines on older builds) or is "rebuild everything together" the policy?
4. **Worktree ownership record.** herdr stores only git provenance on the workspace; theuth's waves need "agent X, task Y, cut from sha Z, target dir T, delete-after-merge". Where does that record live — crates/tasks, the session row, or a `.theuth/worktrees.json`?
5. **No performance numbers were found** for the render stream or ghostty parsing beyond benchmark harnesses (src/server/render_scale_benchmark.rs#L14-L24). If A7-3 (scroll patches) is ever considered, measure theuth's own PTY → desktop byte rate first.
6. **Alt-screen harvest safety for harness PTYs:** herdr sends synthetic wheel events into the app (src/server/alt_screen_read.rs#L16); for omp/claude in a theuth PTY, does a wheel event ever change agent state (e.g. scroll-triggered redraw interpreted as activity by `job_idle`)? Test before borrowing A7-4.
7. **Plugin event hooks vs theuth watchers (design 40).** `on = "worktree.created"` style hooks overlap with sensors/routines; which one should a "bootstrap every new worktree" behaviour use?
8. **What the sibling found on A1–A4** (hook arbitration, live handoff) may change the A6 ranking: if theuth adopts live handoff for engine updates, the restart-reason enum (A6-1) becomes the natural place to plug it in.