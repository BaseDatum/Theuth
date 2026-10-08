import type { UseNavigateResult } from '@tanstack/react-router'
import { toast } from 'sonner'
import { gitUrl, mcpEntry } from '@/lib/mcpEntry'
import { queryKeys } from '@/lib/rpc'
import { openSearch } from '@/lib/search'
import { LOCAL, useEngines } from '@/link/engines'
import type { Api } from '@/rpc/api'
import type { CommunicationState } from '@/rpc/bindings/CommunicationState'
import type { CompactStyle } from '@/rpc/bindings/CompactStyle'
import type { SyncResult } from '@/rpc/bindings/SyncResult'
import type { LoadedPage } from '@/rpc/extra'
import { useConnection } from '@/store/connection'
import { active } from '@/store/engine-scope'
import { useSessions } from '@/store/sessions'
import { EMPTY, useDrafts, useSkillDraft } from '@/lib/drafts'
import { openContextPicker, type ShakeCategory } from './ContextPicker'
import { openBroadcastDialog } from './BroadcastDialog'

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
    desc: "model for subagents spawned without an explicit one (off = the session's model)",
  },
  {
    name: '/effort',
    hint: '[level|clear]',
    desc: 'reasoning effort (picker without arg)',
  },
  {
    name: '/mcp',
    hint: '[add <url|json>|sync]',
    desc: 'MCP servers: on/off (this session or always), risk lines',
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
  { name: '/sessions', hint: '', desc: 'switch session (search, sessions only)' },
  { name: '/resume', hint: '<id>', desc: 'open a session' },
  { name: '/new', hint: '', desc: 'start a new session' },
  {
    name: '/clear',
    hint: '',
    desc: 'drop the conversation, keep the session and its settings',
  },
  {
    name: '/approvals',
    hint: '[on|off] [always]',
    desc: 'tool approval prompts; `always` persists to config.yaml',
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
    desc: 'fold context; style sets the default',
  },
  { name: '/undo', hint: '', desc: 'drop the last turn (redo restores)' },
  { name: '/redo', hint: '', desc: 'restore the last undone turn' },
  { name: '/regen', hint: '', desc: 'regenerate the reply to the last turn' },
  {
    name: '/shake',
    hint: '[images|documents|messages|tools|summary]',
    desc: 'remove exact messages/attachments from the active context (picker)',
  },
  {
    name: '/fork',
    hint: '',
    desc: 'new session with the whole conversation',
  },
  { name: '/quit', hint: '', desc: 'exit' },
  {
    name: '/provider',
    hint: '[add|rm <name>]',
    desc: 'OpenAI-compatible providers',
  },
  {
    name: '/aws',
    hint: '[profile|clear]',
    desc: 'AWS profile Bedrock signs with, node-wide (picker without arg)',
  },
  { name: '/exit', hint: '', desc: 'same as /quit' },
]

/** Commands whose name starts with the typed word (`/mo` → `/model`). */
export const matches = (word: string) => COMMANDS.filter((c) => c.name.startsWith(word))

/**
 * Is `line` a slash command: its first word is a built-in (`COMMANDS`) or names a known skill. Anything else —
 * `/Users/x/y is broken`, `/etc/hosts`, a bare `/` — is message text. `skills === null` (index not loaded yet):
 * a plain `/word` still counts, the agent names an unknown skill; a word holding another `/` or a `.` is a path.
 */
export function isCommand(line: string, skills: readonly string[] | null): boolean {
  const word = line.split(/\s/, 1)[0] ?? ''
  if (word.length < 2 || !word.startsWith('/')) return false
  if (COMMANDS.some((c) => c.name === word)) return true
  const name = word.slice(1)
  return skills ? skills.includes(name) : !/[/.]/.test(name)
}

/** Header surfaces a command can open; the header listens for `theuth:open`. */
export type Surface = 'model' | 'prompt' | 'jobs' | 'todos'

export function openSurface(surface: Surface): void {
  window.dispatchEvent(new CustomEvent('theuth:open', { detail: surface }))
}

/** `theuth:open` detail, when it names a surface. */
export function surfaceOf(e: Event): Surface | null {
  const d: unknown = e instanceof CustomEvent ? e.detail : null
  return d === 'model' || d === 'prompt' || d === 'jobs' || d === 'todos' ? d : null
}

export type SettingsTab =
  | 'general'
  | 'loop'
  | 'approvals'
  | 'communication'
  | 'computer'
  | 'agents'
  | 'link'
  | 'mesh'
  | 'connections'
  | 'channels'
  | 'shortcuts'

type Navigate = UseNavigateResult<string>

/** Open the settings dialog on `tab`; `session` scopes the per-session switches. */
export function settings(navigate: Navigate, tab: SettingsTab, session: string | null | undefined) {
  return navigate({
    to: '/settings/$tab',
    params: { tab },
    search: session ? { session } : {},
  })
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

const STYLES: readonly CompactStyle[] = ['summary', 'cut', 'snap', 'clear']
const style = (s: string) => STYLES.find((x) => x === s)
const CATEGORIES: readonly ShakeCategory[] = ['images', 'documents', 'messages', 'tools', 'summary']

const need = (session: string | null): string => {
  if (!session) throw new Error('no session')
  return session
}

/**
 * What `load_session` would return for a session created a moment ago. Marking
 * a fresh session loaded keeps the route from re-fetching an empty history
 * over the in-flight first turn.
 */
export const FRESH: LoadedPage = {
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
}

/** Park `name` (and any `words` typed after it) on `session`'s composer — creating and opening the session when
 * there is none — and show that chat. No turn starts: the next send hands the skill over with the user's words
 * (`use_skill`). Returns the session shown. */
export async function draftSkill(
  api: Api,
  navigate: Navigate,
  session: string | null,
  name: string,
  words = '',
): Promise<string> {
  const id = session ?? (await api.new_session())
  if (!session) useSessions.getState().loadHistory(id, FRESH, 'replace')
  useSkillDraft.getState().set(id, name)
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
    case '/model':
      if (!arg) return openSurface('model')
      await api.set_model(session, arg, true)
      return
    case '/effort':
      if (!arg) return openSurface('model')
      await api.set_effort(session, arg === 'clear' ? null : arg, true)
      return
    case '/title': {
      if (!arg) return settings(navigate, 'loop', session)
      const m = await api.set_title_model(arg === 'off' || arg === 'clear' ? null : arg)
      active().qc.setQueryData(queryKeys.titleModel, m)
      // 57 §0.9: the escalation model inherits it.
      void active().qc.invalidateQueries({ queryKey: queryKeys.escalateModel })
      toast(m ? `title model: ${m.id}` : 'title model: off')
      return
    }
    // 57 §0.9: tier 2 of the decision oracle; the server rejects ids that are not catalog entries.
    case '/escalate': {
      if (!arg) return settings(navigate, 'loop', session)
      const m = await api.set_escalate_model(arg === 'off' || arg === 'clear' ? null : arg)
      void active().qc.invalidateQueries({ queryKey: queryKeys.escalateModel })
      toast(m ? `escalation model: ${m.id}` : 'escalation model: off (inherits title model)')
      return
    }
    case '/subagent': {
      if (!arg) return settings(navigate, 'loop', session)
      const m = await api.set_subagent_model(arg === 'off' || arg === 'clear' ? null : arg)
      active().qc.setQueryData(queryKeys.subagentModel, m)
      toast(m ? `subagent model: ${m.id}` : 'subagent model: off')
      return
    }
    // 52 §3.2: the Agent loop dropdown's RPC and query; the server rejects unavailable/text-only ids.
    case '/vision': {
      if (!arg) return settings(navigate, 'loop', session)
      const m = await api.set_vision_model(arg === 'off' || arg === 'clear' ? null : arg)
      active().qc.setQueryData(queryKeys.visionModel, m)
      toast(m ? `vision fallback: ${m.id}` : 'vision fallback: off')
      return
    }
    case '/approvals': {
      if (!arg) return settings(navigate, 'approvals', session)
      if (sub !== 'on' && sub !== 'off')
        throw new Error(`/approvals: expected on|off, got ${JSON.stringify(sub)}`)
      if (rest && rest !== 'always')
        throw new Error(`/approvals: expected \`always\`, got ${JSON.stringify(rest)}`)
      if (!rest && !session)
        throw new Error('no session yet: send a message first, or use `/approvals off always`')
      const approvals = await api.set_approvals(rest ? null : session, sub === 'on')
      const { info, setInfo } = useConnection.getState()
      if (info) setInfo({ ...info, approvals })
      return
    }
    case '/communication': {
      if (!arg) return settings(navigate, 'communication', session)
      if (sub !== 'on' && sub !== 'off')
        throw new Error(`/communication: expected on|off, got ${JSON.stringify(sub)}`)
      if (rest && rest !== 'always')
        throw new Error(`/communication: expected \`always\`, got ${JSON.stringify(rest)}`)
      if (!rest && !session)
        throw new Error('no session yet: send a message first, or use `/communication off always`')
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
      need(session)
      return settings(navigate, 'communication', session)
    // Mirror of the TUI entry (62 §5.1): a leading selector word presets the ticks, the rest prefills the message.
    case '/broadcast':
      if (sub === 'running' || sub === 'roots') return openBroadcastDialog({ preset: sub, text: rest })
      return openBroadcastDialog({ text: arg })
    case '/a2a': {
      if (!arg) return settings(navigate, 'communication', session)
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
    case '/mcp':
      if (!arg) return navigate({ to: '/tools', search: session ? { session } : {} })
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
      throw new Error('usage: /mcp [add <url|json> | sync]')
    case '/acp':
      if (!arg) return settings(navigate, 'agents', session)
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
      if (!arg) return settings(navigate, 'connections', session)
      // A provider name first, else an MCP server.
      const url = await api.login(arg).catch(() => api.mcp_login(arg))
      // Over link the daemon opens its own browser on the engine (31 §9); here the URL is only shown.
      if (useEngines.getState().active !== LOCAL) {
        toast.message(
          `Sign in from the node (${useConnection.getState().info?.name ?? 'node'})`,
          { description: url },
        )
      } else {
        window.open(url, '_blank')
      }
      return
    }
    case '/logout':
      if (!arg) throw new Error('usage: /logout <name>')
      await api.logout(arg).catch(() => api.mcp_logout(arg))
      toast(`logged out of ${arg}`)
      return
    case '/provider':
      if (!arg) return settings(navigate, 'connections', session)
      if (sub === 'add') {
        const [name, after] = split(rest)
        const [url, key] = split(after)
        if (!name || !url || !key) return settings(navigate, 'connections', session)
        await api.add_provider(name, url, key)
        toast(`added provider ${name}`)
        return
      }
      if (sub === 'rm' && rest) {
        await api.remove_provider(rest)
        toast(`removed provider ${rest}`)
        return
      }
      throw new Error('usage: /provider [add [name [url [key]]] | rm <name>]')
    case '/aws': {
      if (!arg) return settings(navigate, 'connections', session)
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
      const id = need(session)
      if (!arg) throw new Error('usage: /project <label>|off')
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
      if (!arg) return compact(api, need(session), null)
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
      return compact(api, need(session), 'clear')
    case '/cancel':
      if (session) await api.cancel(session)
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
    // Literal categories only (48 §2.1): an unknown word is usage, never a skill or chat input.
    case '/shake': {
      const id = need(session)
      if (!arg) return openContextPicker(id)
      const c = CATEGORIES.find((x) => x === arg)
      if (!c) throw new Error('usage: /shake [images|documents|messages|tools|summary]')
      return openContextPicker(id, c)
    }
    case '/fork': {
      const nid = await api.fork_session(need(session), null, null)
      await navigate({ to: '/c/$session', params: { session: nid } })
      return
    }
    default: {
      // `/<skill> [args]` hands the skill to the model now (04 §2.4): `isCommand` let only a known skill (or, before
      // the index loads, a plain word the agent may name) through; unknown words are message text, never an error.
      // `/skill:<name>` never reaches here: the composer parks it (`draftSkill`) and keeps the words typed after it.
      const id = session ?? await api.new_session()
      if (!session) useSessions.getState().loadHistory(id, FRESH, 'replace')
      await api.use_skill(id, cmd.slice(1), arg)
      if (!session) await navigate({ to: '/c/$session', params: { session: id } })
    }
  }
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
