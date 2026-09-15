import type { UseNavigateResult } from '@tanstack/react-router'
import { openUrl } from '@tauri-apps/plugin-opener'
import { toast } from 'sonner'
import { gitUrl, mcpEntry } from '@/lib/mcpEntry'
import type { Api } from '@/rpc/api'
import type { CompactStyle } from '@/rpc/bindings/CompactStyle'
import type { SyncResult } from '@/rpc/bindings/SyncResult'
import type { LoadedPage } from '@/rpc/extra'
import { useConnection } from '@/store/connection'
import { useSessions } from '@/store/sessions'

/** Slash commands (`tui::model::COMMANDS`): name, argument hint, description. */
export const COMMANDS: readonly { name: string; hint: string; desc: string }[] = [
  { name: '/model', hint: '[id]', desc: 'switch model (picker without id)' },
  { name: '/title', hint: '[model|off]', desc: 'model that names sessions in the history list (off = the session\'s model)' },
  { name: '/effort', hint: '[level|clear]', desc: 'reasoning effort (picker without arg)' },
  { name: '/mcp', hint: '[add <url|json>|sync]', desc: 'MCP servers: on/off (this session or always), risk lines' },
  { name: '/acp', hint: '[add <registry-id>|remove <name>]', desc: 'External harnesses (ACP) and the registry' },
  { name: '/skills', hint: '[add <url>|rm <name>|sync]', desc: 'skills: picker; `/<skill> [words]` hands one to the model' },
  { name: '/knowledge', hint: '[query | history [id] | restore <id> [commit] | harvest]', desc: 'knowledge base: list, search, history, restore, harvest' },
  { name: '/login', hint: '[name]', desc: 'log in to a provider or MCP server (picker without name)' },
  { name: '/logout', hint: '<name>', desc: 'forget a login' },
  { name: '/prompt', hint: '', desc: 'show the assembled system prompt' },
  { name: '/agents', hint: '', desc: 'subagents of this session' },
  { name: '/jobs', hint: '[kill <id>]', desc: 'list this session\'s background jobs' },
  { name: '/parent', hint: '', desc: 'open the parent session (from a subagent)' },
  { name: '/sessions', hint: '', desc: 'switch session (picker)' },
  { name: '/resume', hint: '<id>', desc: 'open a session' },
  { name: '/new', hint: '', desc: 'start a new session' },
  { name: '/clear', hint: '', desc: 'drop the conversation, keep the session and its settings' },
  { name: '/approvals', hint: '[on|off] [always]', desc: 'tool approval prompts; `always` persists to config.yaml' },
  { name: '/cancel', hint: '', desc: 'abort the in-flight turn' },
  { name: '/compact', hint: '[summary|cut|snap|style …]', desc: 'fold context; style sets the default' },
  { name: '/undo', hint: '', desc: 'drop the last turn (redo restores)' },
  { name: '/redo', hint: '', desc: 'restore the last undone turn' },
  { name: '/regen', hint: '', desc: 'regenerate the reply to the last turn' },
  { name: '/fork', hint: '', desc: 'new session with the whole conversation' },
  { name: '/quit', hint: '', desc: 'exit' },
  { name: '/provider', hint: '[add|rm <name>]', desc: 'OpenAI-compatible providers' },
  { name: '/exit', hint: '', desc: 'same as /quit' },
]

/** Commands whose name starts with the typed word (`/mo` → `/model`). */
export const matches = (word: string) => COMMANDS.filter((c) => c.name.startsWith(word))

/** Header surfaces a command can open; the header listens for `wth:open`. */
export type Surface = 'model' | 'prompt' | 'jobs'

export function openSurface(surface: Surface): void {
  window.dispatchEvent(new CustomEvent('wth:open', { detail: surface }))
}

/** `wth:open` detail, when it names a surface. */
export function surfaceOf(e: Event): Surface | null {
  const d: unknown = e instanceof CustomEvent ? e.detail : null
  return d === 'model' || d === 'prompt' || d === 'jobs' ? d : null
}

export type SettingsTab =
  | 'general'
  | 'approvals'
  | 'tools'
  | 'computer'
  | 'skills'
  | 'harnesses'
  | 'connections'
  | 'channels'
  | 'shortcuts'

type Navigate = UseNavigateResult<string>

/** Open the settings dialog on `tab`; `session` scopes the per-session switches. */
export function settings(navigate: Navigate, tab: SettingsTab, session: string | null | undefined) {
  return navigate({ to: '/settings/$tab', params: { tab }, search: session ? { session } : {} })
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
  running: true,
  model: '',
  effort: null,
  queued: [],
  undone: 0,
  agent: null,
  name: null,
  parent: null,
  skills: [],
  first_seq: null,
  more: false,
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
      window.dispatchEvent(new Event('wth:quit'))
      return
    case '/sessions':
      window.dispatchEvent(new Event('wth:search'))
      return
    case '/resume':
      if (!/^[0-9a-f-]{36}$/i.test(arg)) throw new Error(`not a session id: ${JSON.stringify(arg)}`)
      await navigate({ to: '/c/$session', params: { session: arg } })
      return
    case '/new':
      await navigate({ to: '/' })
      return
    case '/model':
      if (!arg) return openSurface('model')
      await api.set_model(session, arg)
      return
    case '/effort':
      if (!arg) return openSurface('model')
      await api.set_effort(session, arg === 'clear' ? null : arg)
      return
    case '/title': {
      if (!arg) return settings(navigate, 'general', session)
      const m = await api.set_title_model(arg === 'off' || arg === 'clear' ? null : arg)
      toast(m ? `title model: ${m.id}` : 'title model: off')
      return
    }
    case '/approvals': {
      if (!arg) return settings(navigate, 'approvals', session)
      if (sub !== 'on' && sub !== 'off') throw new Error(`/approvals: expected on|off, got ${JSON.stringify(sub)}`)
      if (rest && rest !== 'always') throw new Error(`/approvals: expected \`always\`, got ${JSON.stringify(rest)}`)
      if (!rest && !session) throw new Error('no session yet: send a message first, or use `/approvals off always`')
      const approvals = await api.set_approvals(rest ? null : session, sub === 'on')
      const { info, setInfo } = useConnection.getState()
      if (info) setInfo({ ...info, approvals })
      return
    }
    case '/mcp':
      if (!arg) return settings(navigate, 'tools', session)
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
      if (!arg) return settings(navigate, 'harnesses', session)
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
      if (!arg) return settings(navigate, 'skills', session)
      if (sub === 'sync') return synced(await api.sync_skills())
      if (sub === 'add' && rest) {
        const added = await api.add_skill_repo(rest, null)
        toast(`added ${added.length} skills`)
        return
      }
      if (sub === 'rm' && rest) {
        // A repo dir first (only removed when it holds `.git`), else a local skill.
        const repo = await api.remove_skill_repo(rest).then(() => true, () => false)
        if (!repo) await api.remove_skill(rest)
        toast(repo ? `removed repo ${rest}` : `removed skill ${rest}`)
        return
      }
      throw new Error('usage: /skills [add <url> | rm <name> | sync]')
    case '/knowledge':
      if (!arg) return navigate({ to: '/knowledge' })
      if (sub === 'history') {
        return rest ? navigate({ to: '/knowledge/$', params: { _splat: rest } }) : navigate({ to: '/knowledge' })
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
      await openUrl(url)
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
      for (const e of st.sessions[id]?.entries ?? []) if ((e.kind === 'user' || e.kind === 'assistant') && e.turn !== undefined) turn = e.turn
      if (turn === undefined) throw new Error('nothing to regenerate')
      st.truncate(id, turn)
      await api.regenerate(id, turn).catch(async (e: unknown) => {
        await ctx.reload()
        throw e
      })
      return
    }
    case '/fork': {
      const nid = await api.fork_session(need(session), null, null)
      await navigate({ to: '/c/$session', params: { session: nid } })
      return
    }
    default: {
      // `/<skill> [args]` hands the skill to the model (04 §2.4); the agent names an unknown skill.
      const id = await api.use_skill(session, cmd.slice(1), arg)
      if (!session) {
        useSessions.getState().loadHistory(id, FRESH, 'replace')
        await navigate({ to: '/c/$session', params: { session: id } })
      }
    }
  }
}

async function compact(api: Api, session: string, s: CompactStyle | null) {
  const epoch = await api.compact(session, s)
  // The `Compacted` event wipes the transcript and announces a clear.
  if (s !== 'clear') toast(s ? `context compacted (epoch ${epoch} · ${s})` : `context compacted (epoch ${epoch})`)
}

function synced(results: SyncResult[]) {
  toast(results.length ? results.map((r) => `${r.name}: ${r.status}`).join('\n') : 'nothing to sync')
}
