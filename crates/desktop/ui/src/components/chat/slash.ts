import { receiveComponent } from '@/rpc/bindings/receive'
import type { UseNavigateResult } from '@tanstack/react-router'
import { notify as toast } from '@/lib/notify'
import { limitsArgs, limitValue, probeLine, refreshedText } from '@/components/settings/providers'
import type { SettingsTabId } from '@/components/settings/registry'
import { gitUrl, mcpEntry } from '@/lib/mcpEntry'
import { queryKeys } from '@/lib/rpc'
import { openSearch } from '@/lib/search'
import { LOCAL, hasLocal, useEngines } from '@/link/engines'
import type { Api } from '@/rpc/bindings/client'
import type { CommunicationState } from "@/rpc/bindings/method.agent.output"
import type { HarnessCommand } from "@/rpc/bindings/method.agent.output"
import type { CompactStyle } from "@/rpc/bindings/method.agent.output"
import type { SyncResult } from "@/rpc/bindings/method.agent.output"
import type { Loaded as LoadedPage } from "@/rpc/bindings/method.agent.output"
import { useConnection } from '@/store/connection'
import { active } from '@/store/engine-scope'
import { useSessions } from '@/store/sessions'
import type { AlwaysAskState } from '@/rpc/bindings/method.agent.output'
import { setAlwaysAsk } from '@/lib/always-ask'
import { EMPTY, openDraftSession, useDrafts, useSkillDraft } from '@/lib/drafts'
import { useSuffixes } from '@/lib/suffixes'
import { notebookKeys } from '@/lib/notebooks-rpc'
import { openContextPicker } from './ContextPicker'
import { SHAKE_USAGE, SHAKE_WORDS } from './shake'
import { openBroadcastDialog } from './broadcast'
import { useToolsSheet } from './tools-sheet'
import { onboardingHere } from '@/lib/onboarding/detect'
import { useOnboarding } from '@/lib/onboarding/state'
import { isPopout } from '@/lib/popout'
import { isIsland, isOverlay } from '@/lib/tauri'
import { type PluginCommand, commandsOf, pluginStore, setCommandCard } from '@/lib/plugins'

/** Slash commands (`tui::model::COMMANDS`): name, argument hint, description. */
export const COMMANDS: readonly { name: string; hint: string; desc: string }[] = [
  { name: '/model', hint: '[id]', desc: 'switch model (picker without id)' },
  {
    name: '/title',
    hint: '[model|off]',
    desc: "model that names sessions in the history list (off = the session's model)",
  },
  {
    name: '/vision',
    hint: '[model|off]',
    desc: 'model that inspects images when the main model cannot (off = images are omitted)',
  },
  {
    name: '/escalate',
    hint: '[model|off]',
    desc: 'tier-2 model of the local decision oracle (off = inherits the title model)',
  },
  {
    name: '/subagent',
    hint: '[model|off]',
    desc: 'default model for theuth subagents spawned without an explicit one (off = same as new sessions)',
  },
  {
    name: '/effort',
    hint: '[level|clear]',
    desc: 'reasoning effort (picker without arg)',
  },
  {
    name: '/fractal',
    hint: 'on|off',
    desc: 'switch this session between fractal context and epochs (idle sessions only)',
  },
  {
    name: '/mcp',
    hint: '[add <url|json>|sync]',
    desc: 'MCP servers: on/off (this session or always), risk lines',
  },
  {
    name: '/builtins',
    hint: '',
    desc: 'Built-in tools: on/off for this chat or always',
  },
  {
    name: '/acp',
    hint: '[add <registry-id>|remove <name>]',
    desc: 'External harnesses (ACP) and the registry',
  },
  {
    name: '/skills',
    hint: '[add <url>|rm <name>|sync]',
    desc: 'skills: picker; `/<skill> [words]` hands one to the model',
  },
  {
    name: '/skill:',
    hint: '<name>',
    desc: 'park a skill on the next message (`/<skill> [words]` sends it now)',
  },
  {
    name: '/knowledge',
    hint: '[query | history [id] | restore <id> [commit] | harvest]',
    desc: 'knowledge base: list, search, history, restore, harvest',
  },
  {
    name: '/autoresearch',
    hint: '[<goal> | <slug> | list | pause|resume|stop|stop_now <slug> | steer <slug> <text>]',
    desc: 'autoresearch studies (140): list, open, pause/resume/stop/steer one directly; a goal or anything else hands the skill to the model',
  },
  {
    name: '/login',
    hint: '[name]',
    desc: 'log in to a provider or MCP server (picker without name)',
  },
  { name: '/logout', hint: '<name>', desc: 'forget a login' },
  { name: '/prompt', hint: '', desc: 'show the assembled system prompt' },
  { name: '/agents', hint: '', desc: 'subagents of this session' },
  {
    name: '/jobs',
    hint: '[kill <id>]',
    desc: "list this session's background jobs",
  },
  { name: '/project', hint: '<label>|off', desc: "set or clear this chat's project (51 §3)" },
  { name: '/cost', hint: '', desc: "this session's returned cost breakdown" },
  { name: '/terminals', hint: '', desc: "this session's live terminals" },
  { name: '/todos', hint: '', desc: "toggle this session's todo card (TUI: F2)" },
  {
    name: '/parent',
    hint: '',
    desc: 'open the parent session (from a subagent)',
  },
  { name: '/take', hint: '', desc: 'continue this session on this node (68)' },
  { name: '/search', hint: '[query]', desc: 'search sessions, artifacts and knowledge (prefilled with query)' },
  { name: '/ask', hint: '[question]', desc: 'ask across notebooks (144 §9; this notebook pre-checked on a notebook page)' },
  { name: '/sessions', hint: '', desc: 'switch session (search, sessions only)' },
  { name: '/resume', hint: '<id>', desc: 'open a session' },
  { name: '/new', hint: '', desc: 'start a new session' },
  { name: '/onboarding', hint: '', desc: 'restart the welcome tour (desktop app)' },
  {
    name: '/clear',
    hint: '',
    desc: 'drop the conversation, keep the session and its settings',
  },
  {
    name: '/approvals',
    hint: '[on|off] [always]',
    desc: 'tool approval prompts; `always` persists the node-wide default',
  },
  {
    name: '/always-ask',
    hint: '[<tool> [on|off] [always]]',
    desc: 'a tool always needs your approval, even with approvals off; `always` = every chat (list without args)',
  },
  {
    name: '/communication',
    hint: '[on|off] [always]',
    desc: 'model-to-model messaging; `always` persists the node-wide switch',
  },
  { name: '/peers', hint: '', desc: 'peers this session may message (picker) and pending requests' },
  { name: '/broadcast', hint: '[running|roots] [text]', desc: 'send one message to many sessions' },
  { name: '/a2a', hint: '[reload]', desc: 'network A2A status; `reload` re-reads peers/exports' },
  { name: '/cancel', hint: '', desc: 'abort the in-flight turn' },
  {
    name: '/compact',
    hint: '[summary|cut|snap|style …]',
    desc: 'fold context (endless chat: trim the tail now); style sets the default',
  },
  { name: '/undo', hint: '', desc: 'drop the last turn (redo restores)' },
  { name: '/redo', hint: '', desc: 'restore the last undone turn' },
  { name: '/regen', hint: '', desc: 'regenerate the reply to the last turn' },
  {
    name: '/shake',
    hint: `[${SHAKE_WORDS.join('|')}]`,
    desc: 'remove context by category, group or item (picker)',
  },
  {
    name: '/fork',
    hint: '',
    desc: 'new session with the whole conversation',
  },
  { name: '/quit', hint: '', desc: 'exit' },
  {
    name: '/provider',
    hint: '[add|find|limits|refresh|rm]',
    desc: 'providers (built-in, local and custom)',
  },
  {
    name: '/aws',
    hint: '[profile|clear]',
    desc: 'AWS profile Bedrock signs with, node-wide (picker without arg)',
  },
  { name: '/tools', hint: '[add <url|json>|sync]', desc: 'same as /mcp' },
  { name: '/exit', hint: '', desc: 'same as /quit' },
]

/** Commands whose name starts with the typed word (`/mo` → `/model`). */
export const matches = (word: string) => COMMANDS.filter((c) => c.name.startsWith(word))

/** Old names still run but are not listed: `/endless` is `/fractal`'s. */
const ALIASES: readonly string[] = ['/endless']

/** One slash-menu row: a theuth built-in or, in an ACP session (D15), one of the harness's own commands. */
export type SlashItem = {
  name: string
  hint: string
  desc: string
  /** `theuth` or the harness's name; set only when both kinds are listed. */
  source?: string
  /** Picked → the name goes to the harness verbatim (`sendRaw`), never through theuth's parsing. */
  raw?: boolean
}

/** What the composer needs to list a harness's commands: the ACP child's agent name, its manifest, theuth's skill names (`null` until the index loads). */
export type HarnessMenu = { agent: string; commands: readonly HarnessCommand[]; skills: readonly string[] | null }

/**
 * D15 (b): the slash menu's rows for the typed `word`. theuth's built-ins first; in an ACP session the harness's
 * commands follow, every row labelled with its source. A harness `skill:<name>` that theuth also has as a skill is
 * listed twice, theuth's row first — typed `/skill:<name>` ⏎ keeps parking theuth's skill, as the hint says; the
 * harness's row is sent raw when picked. 216 §9.1: running plugins' `/<plugin>[:<cmd>]` close the list, labelled
 * `plugin · <name>`; a name a built-in or the harness already lists stays theirs; a hidden one is left out (it still runs typed).
 */
export function slashMenu(word: string, harness: HarnessMenu | null | undefined, plugins: readonly PluginCommand[] = []): SlashItem[] {
  const own = matches(word)
  const items: SlashItem[] = harness ? own.map((c) => ({ ...c, source: 'theuth' })) : own
  if (harness)
    for (const c of harness.commands) {
      const name = `/${c.name}`
      if (!name.startsWith(word)) continue
      const skill = c.name.startsWith('skill:') ? c.name.slice('skill:'.length) : null
      if (skill && harness.skills?.includes(skill)) items.push({ name, hint: '[words]', desc: `park theuth's ${skill} skill on the next message (typed ⏎)`, source: 'theuth' })
      items.push({ name, hint: c.hint ?? '', desc: `${c.description}${skill && harness.skills?.includes(skill) ? ` (${harness.agent}'s — picked, sent as typed)` : ''}`, source: harness.agent, raw: true })
    }
  const taken = new Set(items.map((i) => i.name))
  for (const { plugin, command, hidden } of plugins) {
    if (hidden || !command.slash.startsWith(word) || taken.has(command.slash)) continue
    items.push({ name: command.slash, hint: command.args ?? '', desc: command.description, source: `plugin · ${plugin}` })
  }
  return items
}

/**
 * D15: `text` goes to `session` verbatim — a harness command (`/skill:ponytail`), never parsed as theuth's (`/skill:`
 * parking, built-ins). Queued (`steer`) mid-turn like any send; shown at once like one.
 */
export async function sendRaw(api: Api, session: string, text: string): Promise<void> {
  const st = useSessions.getState()
  const id = crypto.randomUUID()
  if (st.sessions[session]?.running) {
    st.applyEvents([{ type: 'InputQueued', session, id, text, defer: false }])
    await api.steer(session, text, id, [])
  } else {
    st.markSent(session, text, [])
    await api.send_input(session, text, [], null, id)
  }
}

/**
 * Is `line` a slash command: its first word is a built-in (`COMMANDS`), a running plugin's command (216), or names a
 * known skill. Anything else — `/Users/x/y is broken`, `/etc/hosts`, a bare `/` — is message text. `skills === null`
 * (index not loaded yet): a plain `/word` still counts, the agent names an unknown skill; a word holding another `/`
 * or a `.` is a path.
 */
export function isCommand(line: string, skills: readonly string[] | null, plugins: readonly PluginCommand[] = []): boolean {
  const word = line.split(/\s/, 1)[0] ?? ''
  if (word.length < 2 || !word.startsWith('/')) return false
  if (ALIASES.includes(word) || COMMANDS.some((c) => c.name === word)) return true
  if (plugins.some((p) => p.command.slash === word)) return true
  const name = word.slice(1)
  return skills ? skills.includes(name) : !/[/.]/.test(name)
}

/** Surfaces a command can open via `theuth:open`: the header's, and (215 C1) `model`, the composer's `ComposerModel`. */
export type Surface = 'model' | 'prompt' | 'jobs' | 'todos' | 'memory'

export function openSurface(surface: Surface): void {
  window.dispatchEvent(new CustomEvent('theuth:open', { detail: surface }))
}

/** `theuth:open` detail, when it names a surface. */
export function surfaceOf(e: Event): Surface | null {
  const d: unknown = e instanceof CustomEvent ? e.detail : null
  return d === 'model' || d === 'prompt' || d === 'jobs' || d === 'todos' || d === 'memory' ? d : null
}

type Navigate = UseNavigateResult<string>

/** Open Settings on `tab`; `focus` names a control the tab focuses (Memory: `model` | `spend`). */
export function settings(navigate: Navigate, tab: SettingsTabId, focus?: string) {
  return navigate({ to: '/settings/$tab', params: { tab }, search: focus ? { focus } : {} })
}

export type Ctx = {
  api: Api
  session: string | null
  parent: string | null | undefined
  navigate: Navigate
  /** Re-fetch the visible session after `undo`/`redo` (the TUI reloads there too). */
  reload(): Promise<void>
}

const split = (s: string): [string, string] => {
  const at = s.indexOf(' ')
  return at < 0 ? [s, ''] : [s.slice(0, at), s.slice(at + 1).trim()]
}

/** 177 U6: `/provider limits`; `0` = unknown, an omitted output keeps the stored one. */
const LIMITS_USAGE = 'usage: /provider limits <provider> <model> <context> [output]'

/** 177 R1-4: a provider or its limits changed — Connections, the pickers and the context meter refetch, as the settings forms do. */
function providersChanged() {
  const qc = active().qc
  void qc.invalidateQueries({ queryKey: queryKeys.providers })
  void qc.invalidateQueries({ queryKey: queryKeys.models })
}

const STYLES: readonly CompactStyle[] = ['summary', 'cut', 'snap', 'clear']
const style = (s: string) => STYLES.find((x) => x === s)

const need = (session: string | null): string => {
  if (!session) throw new Error('no session')
  return session
}

/** 219 §6.2: `/always-ask <tool> [on|off] [always]`; `on` by default, `always` = harness-wide. */
export function parseAlwaysAsk(arg: string): { tool: string; on: boolean; always: boolean } {
  const [tool, ...words] = arg.split(/\s+/).filter(Boolean)
  if (!tool || tool === 'on' || tool === 'off' || tool === 'always') throw new Error('usage: /always-ask <tool> [on|off] [always]')
  let on = true
  let always = false
  words.forEach((w, i) => {
    if ((w === 'on' || w === 'off') && i === 0) on = w === 'on'
    else if (w === 'always' && i === words.length - 1) always = true
    else throw new Error(`/always-ask: expected on|off then \`always\`, got ${JSON.stringify(w)}`)
  })
  return { tool, on, always }
}

/** The bare `/always-ask` reading, one scope per clause. */
export function alwaysAskText(s: AlwaysAskState): string {
  const parts = [
    s.global.length ? `always: ${s.global.join(', ')}` : '',
    s.session.length ? `this chat: ${s.session.join(', ')}` : '',
    s.inherited.length ? `inherited: ${s.inherited.join(', ')}` : '',
  ].filter(Boolean)
  return parts.length ? `always ask · ${parts.join(' · ')}` : 'always ask: no tools'
}

/**
 * What `load_session` would return for a session created a moment ago. Marking
 * a fresh session loaded keeps the route from re-fetching an empty history
 * over the in-flight first turn.
 */
export const FRESH: LoadedPage = receiveComponent("method.agent.output.Loaded", {
  history: [],
  running: false,
  model: '',
  effort: null,
  queued: [],
  undone: 0,
  agent: null,
  name: null,
  parent: null,
  skills: [],
  harness_commands: [],
  first_seq: null,
  more: false,
  context_revision: '0',
  context_editable: true,
  context_reason: null,
  context_estimate: null,
  settled: true,
  todos: null,
  communication: null,
  project: null,
  retention: 'durable',
})

/** Park `name` (and any `words` typed after it) on `session`'s composer, after any skills already parked there —
 * creating and opening the session when there is none (`openDraftSession`: a draft model/effort pick rides along,
 * 148 R1) — and show that chat. No turn starts: the next send hands the skills over with the user's words
 * (`sendSkills`). Returns the session shown. */
export async function draftSkill(
  api: Api,
  navigate: Navigate,
  session: string | null,
  name: string,
  words = '',
  suffixFrom?: string,
): Promise<string> {
  const id = session ?? (await openDraftSession(api)).id
  if (!session) useSessions.getState().loadHistory(id, FRESH, 'replace')
  useSkillDraft.getState().add(id, name)
  // A new chat: the prompt suffix turned on in the draft (`suffixFrom`, its key) goes with it, like the words, so the
  // first message carries skill and suffix together instead of leaving the suffix for the next new chat.
  if (!session && suffixFrom !== undefined) useSuffixes.getState().carry(suffixFrom, id)
  // Before `navigate`: the chat's composer reads its text from the draft store as it mounts.
  useDrafts.getState().set(id, { text: words, pastes: EMPTY.pastes })
  await navigate({ to: '/c/$session', params: { session: id } })
  return id
}

/**
 * Run one `/command` line the way the TUI does (`Model::submit`): the same
 * `Api` call, results as toasts; commands that open a TUI menu open the
 * desktop surface instead. Throws with a user-facing message on bad usage.
 */
export async function run(line: string, ctx: Ctx): Promise<void> {
  const [cmd, arg] = split(line.trim())
  const { api, session, navigate } = ctx
  const [sub, rest] = split(arg)
  switch (cmd) {
    case '/quit':
    case '/exit':
      window.dispatchEvent(new Event('theuth:quit'))
      return
    case '/search':
      openSearch(arg ? { query: arg } : undefined)
      return
    // 144 §9: the Ask dialog is `?ask` on the notebook page (the current notebook pre-checked) or on Library › Notebooks (all notebooks).
    case '/ask': {
      const nb = /^\/notebooks\/([^/?#]+)/.exec(location.pathname)
      const ask = arg || true
      if (nb?.[1]) await navigate({ to: '/notebooks/$id', params: { id: decodeURIComponent(nb[1]) }, search: { ask } })
      else await navigate({ to: '/notebooks', search: { ask } })
      return
    }
    // A one-open Sessions-only override (49 §6): the remembered category selection stays put.
    case '/sessions':
      openSearch({ categories: ['sessions'] })
      return
    case '/resume':
      if (!/^[0-9a-f-]{36}$/i.test(arg)) throw new Error(`not a session id: ${JSON.stringify(arg)}`)
      await navigate({ to: '/c/$session', params: { session: arg } })
      return
    case '/new':
      await navigate({ to: '/new' })
      return
    // 215: as Settings › General › App › Restart the welcome tour; the wizard exists only where `OnboardingHost` runs.
    case '/onboarding':
      if (!onboardingHere({ hasLocal: hasLocal(), active: useEngines.getState().active, local: LOCAL, popout: isPopout(), island: isIsland(), overlay: isOverlay() })) {
        toast.message('Open the desktop app on this computer to restart onboarding.')
        return
      }
      useOnboarding.getState().reset()
      useOnboarding.getState().openWizard('welcome')
      return
    // 148 R1: a pick is this chat's only — the engine switches the session; with no session yet the draft keeps
    // it until the first send creates the session (never `set_model(null, …)`, which would set the default).
    case '/model':
      if (!arg) return openSurface('model')
      if (!session) return useDrafts.getState().setPick({ model: arg })
      await api.set_model(session, arg, true)
      return
    case '/effort':
      if (!arg) return openSurface('model')
      if (!session) return useDrafts.getState().setPick({ effort: arg === 'clear' ? null : arg })
      await api.set_effort(session, arg === 'clear' ? null : arg, true)
      return
    // 137 §8.6 (157 D3): the TUI's `/fractal on|off` on the desktop too — `set_session_memory`, idle sessions only; the
    // badge and the Context panel follow the refreshed `sessions` row (and a notebook's `notebook_chats` row) and `session_memory`.
    case '/fractal':
    case '/endless': {
      if (sub !== 'on' && sub !== 'off') throw new Error(`/fractal: expected on|off, got ${JSON.stringify(sub)}`)
      if (!session) return useDrafts.getState().setCreate({ memory: sub === 'on' ? 'endless' : 'epochs' })
      const m = await api.set_session_memory(session, sub === 'on' ? 'endless' : 'epochs')
      active().qc.setQueryData(queryKeys.sessionMemory(session), m)
      void active().qc.invalidateQueries({ queryKey: queryKeys.sessions })
      void active().qc.invalidateQueries({ queryKey: [...notebookKeys.all, 'chats'] })
      toast(m.mode === 'endless' ? 'Fractal context on' : 'Fractal context off: back to epochs')
      return
    }
    case '/title': {
      if (!arg) return settings(navigate, 'helpers')
      const m = await api.set_title_model(arg === 'off' || arg === 'clear' ? null : arg)
      active().qc.setQueryData(queryKeys.titleModel, m)
      // 57 §0.9: the escalation model inherits it.
      void active().qc.invalidateQueries({ queryKey: queryKeys.escalateModel })
      // 194 D4c: the engine may have dropped the title effort.
      void active().qc.invalidateQueries({ queryKey: queryKeys.titleEffort })
      toast(m ? `title model: ${m.id}` : 'title model: off')
      return
    }
    // 57 §0.9: tier 2 of the decision oracle; the server rejects ids that are not catalog entries.
    case '/escalate': {
      if (!arg) return settings(navigate, 'helpers')
      const m = await api.set_escalate_model(arg === 'off' || arg === 'clear' ? null : arg)
      void active().qc.invalidateQueries({ queryKey: queryKeys.escalateModel })
      void active().qc.invalidateQueries({ queryKey: queryKeys.escalateEffort })
      toast(m ? `escalation model: ${m.id}` : 'escalation model: off (inherits title model)')
      return
    }
    case '/subagent': {
      if (!arg) return settings(navigate, 'models')
      const m = await api.set_subagent_model(arg === 'off' || arg === 'clear' ? null : arg)
      active().qc.setQueryData(queryKeys.subagentModel, m)
      // 194 D4c: the engine may have dropped the subagent effort.
      void active().qc.invalidateQueries({ queryKey: queryKeys.subagentEffort })
      toast(m ? `subagent model: ${m.id}` : 'subagent model: same as new sessions')
      return
    }
    // 52 §3.2: the Agent loop dropdown's RPC and query; the server rejects unavailable/text-only ids.
    case '/vision': {
      if (!arg) return settings(navigate, 'helpers')
      const m = await api.set_vision_model(arg === 'off' || arg === 'clear' ? null : arg)
      active().qc.setQueryData(queryKeys.visionModel, m)
      void active().qc.invalidateQueries({ queryKey: queryKeys.visionEffort })
      toast(m ? `vision fallback: ${m.id}` : 'vision fallback: off')
      return
    }
    case '/approvals': {
      if (!arg) return settings(navigate, 'approvals')
      if (sub !== 'on' && sub !== 'off')
        throw new Error(`/approvals: expected on|off, got ${JSON.stringify(sub)}`)
      if (rest && rest !== 'always')
        throw new Error(`/approvals: expected \`always\`, got ${JSON.stringify(rest)}`)
      if (!rest && !session) return useDrafts.getState().setSettings({ approvals: sub === 'on' })
      const approvals = await api.set_approvals(rest ? null : session, sub === 'on')
      const { info, setInfo } = useConnection.getState()
      if (info) setInfo({ ...info, approvals })
      return
    }
    case '/always-ask': {
      if (!arg) {
        const state = await api.get_always_ask(session)
        const draft = session ? [] : (useDrafts.getState().settings.alwaysAsk ?? [])
        toast(alwaysAskText({ ...state, session: [...new Set([...state.session, ...draft])].sort() }))
        return
      }
      const { tool, on, always } = parseAlwaysAsk(arg)
      await setAlwaysAsk(api, always ? null : (session ?? ''), tool, on)
      void active().qc.invalidateQueries({ queryKey: ['always-ask'] })
      toast(`${tool}: ${on ? 'always asks' : 'no longer always asks'} ${always ? 'in every chat' : 'in this chat'}`)
      return
    }
    case '/communication': {
      if (!arg) return settings(navigate, 'messaging')
      if (sub !== 'on' && sub !== 'off')
        throw new Error(`/communication: expected on|off, got ${JSON.stringify(sub)}`)
      if (rest && rest !== 'always')
        throw new Error(`/communication: expected \`always\`, got ${JSON.stringify(rest)}`)
      if (!rest && !session) return useDrafts.getState().setSettings({ communication: sub === 'on' })
      const target = rest ? null : session
      const state = await api.set_communication(target, sub === 'on')
      active().qc.setQueryData(queryKeys.communication(target), state)
      if (!target) {
        const { info, setInfo } = useConnection.getState()
        if (info) setInfo({ ...info, communication: state })
      }
      toast(communicationText(state))
      return
    }
    case '/peers':
      // 188: a chat's peer requests live in its Messaging… sheet.
      return navigate({ to: '/c/$session', params: { session: need(session) }, search: { messaging: '1' } })
    // Mirror of the TUI entry (62 §5.1): a leading selector word presets the ticks, the rest prefills the message.
    case '/broadcast':
      if (sub === 'running' || sub === 'roots') return openBroadcastDialog({ preset: sub, text: rest })
      return openBroadcastDialog({ text: arg })
    case '/a2a': {
      if (!arg) return settings(navigate, 'messaging')
      if (sub !== 'reload') throw new Error('usage: /a2a [reload]')
      const status = await api.reload_a2a()
      active().qc.setQueryData(queryKeys.a2a, status)
      toast(
        status.error
          ? `a2a: reload failed, previous config kept: ${status.error}`
          : `a2a: ${status.peers.length} peers, ${status.exports.length} exports${status.listening ? ` · listening on ${status.address ?? '?'}` : ''}`,
      )
      return
    }
    // Both an open chat and the draft have their own pending "Off here" switches. /mcp opens the MCP tab, /tools the
    // tab last shown, /builtins the Builtins tab.
    case '/builtins':
      if (arg) throw new Error('usage: /builtins')
      return useToolsSheet.getState().show(session ?? '', 'builtins')
    case '/mcp':
    case '/tools':
      if (!arg) return useToolsSheet.getState().show(session ?? '', cmd === '/mcp' ? 'mcp' : undefined)
      if (sub === 'sync') return synced(await api.sync_mcp())
      if (sub === 'add' && rest) {
        if (gitUrl(rest)) {
          const repo = await api.add_mcp_repo(rest, null)
          toast(`added repo ${repo.name}`)
          return
        }
        const { name, entry } = mcpEntry(rest)
        await api.add_mcp_server(name, entry)
        toast(`added ${name}`)
        return
      }
      throw new Error(`usage: ${cmd} [add <url|json> | sync]`)
    case '/acp':
      if (!arg) return settings(navigate, 'harnesses')
      if (sub === 'add' && rest) {
        const e = await api.add_acp(rest)
        toast(`added ${e.name}`)
        return
      }
      if (sub === 'remove' && rest) {
        await api.remove_acp(rest)
        toast(`removed ${rest}`)
        return
      }
      throw new Error('usage: /acp [add <registry-id> | remove <name>]')
    case '/skills':
      if (!arg) return navigate({ to: '/skills', search: session ? { session } : {} })
      if (sub === 'sync') return synced(await api.sync_skills())
      if (sub === 'add' && rest) {
        const added = await api.add_skill_repo(rest, null)
        toast(`added ${added.length} skills`)
        return
      }
      if (sub === 'rm' && rest) {
        // A repo dir first (only removed when it holds `.git`), else a local skill.
        const repo = await api.remove_skill_repo(rest).then(
          () => true,
          () => false,
        )
        if (!repo) await api.remove_skill(rest)
        toast(repo ? `removed repo ${rest}` : `removed skill ${rest}`)
        return
      }
      throw new Error('usage: /skills [add <url> | rm <name> | sync]')
    case '/autoresearch': {
      // §13.1: list/status/pause/resume/stop/steer are direct RPC; anything else (a goal, a control in words) is the skill.
      if (!arg || sub === 'list' || sub === 'status') return navigate({ to: '/autoresearch' })
      if (sub === 'pause' || sub === 'resume' || sub === 'stop' || sub === 'stop_now' || sub === 'stop-now') {
        if (!rest) throw new Error(`usage: /autoresearch ${sub} <slug>`)
        const op = sub === 'pause' ? 'pause' : sub === 'resume' ? 'resume' : sub === 'stop' ? 'stop_after' : 'stop_now'
        const info = await api['autoresearch.control'](rest, { op })
        toast(`${info.slug}: ${info.state.state}`)
        return
      }
      if (sub === 'steer') {
        const [slug, text] = split(rest)
        if (!slug || !text) throw new Error('usage: /autoresearch steer <slug> <text>')
        await api['autoresearch.steer'](slug, text)
        toast(`${slug}: steer queued for the next experiment`)
        return
      }
      // One word that names a study opens it.
      if (!rest && (await api['autoresearch.list']()).some((s) => s.slug === sub)) return navigate({ to: '/autoresearch/$slug', params: { slug: sub }, search: {} })
      const id = session ?? (await api.new_session())
      if (!session) useSessions.getState().loadHistory(id, FRESH, 'replace')
      await api.use_skill(id, 'autoresearch', arg)
      if (!session) await navigate({ to: '/c/$session', params: { session: id } })
      return
    }
    case '/knowledge':
      if (!arg) return navigate({ to: '/knowledge' })
      if (sub === 'history') {
        return rest
          ? navigate({ to: '/knowledge/$', params: { _splat: rest }, search: { history: true } })
          : navigate({ to: '/knowledge', search: { history: true } })
      }
      if (sub === 'restore') {
        if (!rest) throw new Error('usage: /knowledge restore <id> [commit]')
        const [id, commit] = split(rest)
        const change = await api.knowledge_restore(id, commit || null)
        toast(`restored ${id} (${change.commit.slice(0, 7)})`)
        return
      }
      if (sub === 'harvest') {
        const r = await api.knowledge_harvest(need(session))
        toast(`knowledge +${r.added} ~${r.updated}`)
        return
      }
      return navigate({ to: '/knowledge', search: { q: arg } })
    case '/login': {
      if (!arg) return settings(navigate, 'providers')
      // A provider name first, else an MCP server.
      const url = await api.login(arg).catch(() => api.mcp_login(arg))
      // The engine opens the browser on its own host (here, or the node over link, 31 §9); here the URL is only shown.
      toast.message(
        useEngines.getState().active === LOCAL ? "Browser didn't open?" : `Sign in from the node (${useConnection.getState().info?.name ?? 'node'})`,
        { description: url },
      )
      return
    }
    case '/logout':
      if (!arg) throw new Error('usage: /logout <name>')
      await api.logout(arg).catch(() => api.mcp_logout(arg))
      toast(`logged out of ${arg}`)
      return
    case '/provider':
      if (!arg) return settings(navigate, 'providers')
      if (sub === 'add') {
        const [name, after] = split(rest)
        const [url, key] = split(after)
        if (!name || !url) return settings(navigate, 'providers')
        // 177: the key is optional; the engine refuses a missing one for a non-local URL ("api key is empty").
        await api.add_provider(name, url, key || null)
        providersChanged()
        toast(`added provider ${name}`)
        return
      }
      if (sub === 'find') {
        const servers = await api.local_servers_probe()
        toast.message('Local servers', { description: servers.map(probeLine).join('\n') || `nothing answered on ${useConnection.getState().info?.name ?? 'this node'}` })
        return
      }
      if (sub === 'limits') {
        const [provider, after] = split(rest)
        const [model, nums] = split(after)
        const [ctxRaw, outRaw] = split(nums)
        // `0` = unknown, as the engine reports it.
        const limit = (s: string) => (s === '0' ? null : limitValue(s))
        const context = limit(ctxRaw)
        const output = outRaw ? limit(outRaw) : undefined
        if (!provider || !model || !ctxRaw || context === undefined || (outRaw && output === undefined)) throw new Error(LIMITS_USAGE)
        // No output given: keep the stored one.
        const keep = output === undefined ? ((await api.provider_models(provider)).find((m) => m.id === model)?.output ?? 0) || null : output
        const m = await api.set_model_limits(...limitsArgs(provider, { id: model, context, output: keep }))
        providersChanged()
        toast(`${provider}/${m.id}: context ${m.context ? m.context.toLocaleString('en-US') : 'unknown'} · output ${m.output ? m.output.toLocaleString('en-US') : 'unknown'}`)
        return
      }
      if (sub === 'refresh') {
        if (!rest) return settings(navigate, 'providers')
        toast(refreshedText(await api.refresh_provider_models(rest)))
        providersChanged()
        return
      }
      if (sub === 'rm' && rest) {
        await api.remove_provider(rest)
        providersChanged()
        toast(`removed provider ${rest}`)
        return
      }
      throw new Error('usage: /provider [add <name> <url> [key] | find | limits <provider> <model> <context> [output] | refresh <name> | rm <name>]')
    case '/aws': {
      if (!arg) return settings(navigate, 'providers')
      const r = await api.set_aws_profile(arg === 'clear' ? null : arg)
      active().qc.setQueryData(queryKeys.awsProfiles, r)
      toast(r.selected ? `bedrock: profile ${r.selected}` : `bedrock: AWS default chain (${r.default_profile})`)
      return
    }
    case '/prompt':
      return openSurface('prompt')
    case '/agents':
      need(session)
      toast('Subagents are listed under this chat in the sidebar')
      return
    case '/parent':
      if (!ctx.parent) throw new Error('this is the top-level session')
      await navigate({ to: '/c/$session', params: { session: ctx.parent } })
      return
    case '/take': {
      const o = await api.session_take(need(session), 'user')
      toast(`session continues here (epoch ${o.epoch})`)
      return
    }
    case '/todos':
      need(session)
      return openSurface('todos')
    case '/jobs':
      if (!arg) {
        need(session)
        return openSurface('jobs')
      }
      if (sub === 'kill' && rest) {
        await api.cancel_job(need(session), rest)
        toast(`killed ${rest}`)
        return
      }
      throw new Error('usage: /jobs [kill <id>]')
    case '/project': {
      if (!arg) throw new Error('usage: /project <label>|off')
      if (!session) return useDrafts.getState().setCreate({ project: arg === 'off' ? null : arg })
      const id = session
      await api.set_session_project(id, arg === 'off' ? null : arg)
      void active().qc.invalidateQueries({ queryKey: queryKeys.sessions })
      toast(arg === 'off' ? 'project cleared' : `project: ${arg}`)
      return
    }
    case '/cost':
      window.dispatchEvent(new CustomEvent('theuth:open-cost', { detail: { session: need(session) } }))
      return
    case '/terminals':
      window.dispatchEvent(new CustomEvent('theuth:terminals', { detail: { session: need(session) } }))
      return
    case '/compact': {
      if (!arg) return contextCommand(api, need(session), '/compact')
      const s = style(sub)
      if (s) return compact(api, need(session), s)
      if (sub === 'style') {
        if (!rest) {
          toast(`compact style: ${await api.compact_style()}`)
          return
        }
        const d = style(rest)
        if (d) {
          await api.set_compact_style(d)
          toast(`compact style: ${d}`)
          return
        }
      }
      throw new Error('usage: /compact [summary|cut|snap|style …]')
    }
    case '/clear':
      return contextCommand(api, need(session), '/clear')
    case '/cancel':
      if (session) await api.cancel(session, null, 'stop')
      return
    case '/undo':
    case '/redo': {
      const id = need(session)
      toast(await (cmd === '/undo' ? api.undo(id) : api.redo(id)))
      await ctx.reload()
      return
    }
    // The last turn, as the TUI's default (29 §5.4): the store drops it first and the
    // stream repaints it; a refusal (`turn in flight`) reloads what was dropped.
    case '/regen': {
      const id = need(session)
      const st = useSessions.getState()
      let turn: number | undefined
      for (const e of st.sessions[id]?.entries ?? [])
        if ((e.kind === 'user' || e.kind === 'assistant') && e.turn !== undefined) turn = e.turn
      if (turn === undefined) throw new Error('nothing to regenerate')
      st.truncate(id, turn)
      await api.regenerate(id, turn).catch(async (e: unknown) => {
        await ctx.reload()
        throw e
      })
      return
    }
    // Literal categories only (48 §2.1, 197 §7): an unknown word is usage, never a skill or chat input.
    case '/shake': {
      const id = need(session)
      if (!arg) return contextCommand(api, id, '/shake')
      const w = SHAKE_WORDS.find((x) => x === arg)
      if (!w) throw new Error(SHAKE_USAGE)
      return openContextPicker(id, w)
    }
    case '/fork': {
      const nid = await api.fork_session(need(session), null, null)
      await navigate({ to: '/c/$session', params: { session: nid } })
      return
    }
    default: {
      // 216 §9.1: a running plugin's `/<plugin>[:<cmd>]` (the cases above, the built-ins, win). The reply's card shows
      // above the composer, attributed; a text-only reply is a note.
      const plugin = commandsOf(pluginStore().getState().plugins).find((c) => c.command.slash === cmd)
      if (plugin) {
        const reply = await api.plugin_command(plugin.plugin, plugin.command.name, arg, session)
        if (reply.ui) setCommandCard({ plugin: plugin.plugin, slash: cmd, session, ui: reply.ui })
        else if (reply.text) toast(reply.text)
        return
      }
      // `/<skill> [args]` hands the skill to the model now (04 §2.4): `isCommand` let only a known skill (or, before
      // the index loads, a plain word the agent may name) through; unknown words are message text, never an error.
      // `/skill:<name>` never reaches here: the composer parks it (`draftSkill`) and keeps the words typed after it.
      const id = session ?? (await openDraftSession(api)).id
      if (!session) useSessions.getState().loadHistory(id, FRESH, 'replace')
      await api.use_skill(id, cmd.slice(1), arg)
      if (!session) await navigate({ to: '/c/$session', params: { session: id } })
    }
  }
}

/** `/compact`, `/shake` and `/clear` as typed without an argument: the composer's context button runs the same. */
export function contextCommand(api: Api, session: string, cmd: '/compact' | '/shake' | '/clear'): void {
  if (cmd === '/shake') return openContextPicker(session)
  compact(api, session, cmd === '/clear' ? 'clear' : null)
}

/** `/compact <style>`: the context button's per-style compact buttons. */
export function compactWith(api: Api, session: string, style: Exclude<CompactStyle, 'clear'>): void {
  compact(api, session, style)
}

// A command whose RPC waits on a model call must not be awaited by `submit` (it holds `busy`, locking the
// composer for the whole compaction): fire, flip `compacting` so a send steers/queues (07 §8.6), settle on failure.
function compact(api: Api, session: string, s: CompactStyle | null): void {
  const store = useSessions.getState()
  store.applyEvents([{ type: 'Compacting', session, turn: null }])
  api.compact(session, s).then(
    (epoch) => {
      // The `Compacted` event wipes the transcript and announces a clear.
      if (s !== 'clear')
        toast(s ? `context compacted (epoch ${epoch} · ${s})` : `context compacted (epoch ${epoch})`)
    },
    (e: unknown) => {
      store.settleCompacting(session)
      toast.error(e instanceof Error ? e.message : String(e))
    },
  )
}

function synced(results: SyncResult[]) {
  toast(
    results.length ? results.map((r) => `${r.name}: ${r.status}`).join('\n') : 'nothing to sync',
  )
}

/** One line for `/communication on|off`: the effective state and its exact blocker (50 §2.1). */
function communicationText(s: CommunicationState): string {
  if (s.effective) return `communication: on${s.session ? '' : ' (global)'}`
  if (!s.global) return `communication: off — disabled globally (${s.source}); use \`/communication on always\``
  if (s.inherited_from && s.inherited_from !== s.session)
    return `communication: off — opted out by ancestor ${s.inherited_from}`
  return 'communication: off — this session opted out (until restart)'
}
