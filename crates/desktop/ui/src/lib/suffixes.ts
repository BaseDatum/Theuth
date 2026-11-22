// #3 Prompt suffixes: named blocks of text the user saves once (Settings › Prompt suffixes) and turns on in the
// composer's `+` menu for one chat — for the next message only (`once`) or for every message until turned off
// (`always`). A send appends the active suffix's text to the outgoing message after a blank line, so it is part of
// the user's message in the transcript. Slash and skill commands never get it. The composer is the one place that
// appends it (`withSuffix`), on every platform.
//
// Where they live: on an engine with the suffix methods (`SUFFIX_METHODS`; `useSuffixSync` connects the store) the
// saved list is the engine's, replicated to every machine of the mesh, and a session's `always` suffix is stored with
// the session on the engine that hosts it, so the phone sees what the desktop turned on. `once` activations and
// new-chat drafts stay here. On an older engine everything stays in `localStorage` as before, and a store that once
// had a local list uploads it to the first supporting engine (by id, so never twice).
// `active` is keyed by chat: the session id, or the composer's draft key (`''`, `panel:`) for a chat not created
// yet, which `carry` moves onto the created session.

import { create } from 'zustand'

const KEY = 'prompt-suffixes'
/** Set once the local list went up to an engine: it is never uploaded again (a suffix deleted since stays deleted). */
const MIGRATED = 'prompt-suffixes-migrated'

export type Suffix = { id: string; name: string; text: string }
/** `once`: the next send only; `always`: every send in that chat until turned off. */
export type SuffixScope = 'once' | 'always'
export type ActiveSuffix = { id: string; scope: SuffixScope }

/** The engine methods the store needs; `useSuffixSync` gates on all of them. */
export const SUFFIX_METHODS = ['list_suffixes', 'put_suffix', 'delete_suffix', 'session_suffix', 'set_session_suffix'] as const

/** The slice of the engine `Api` the store calls. */
export type SuffixEngine = {
  list_suffixes(): Promise<Suffix[]>
  put_suffix(suffix: Suffix): Promise<boolean>
  delete_suffix(id: string): Promise<boolean>
  session_suffix(session: string): Promise<string | null>
  set_session_suffix(session: string, suffix: string | null): Promise<null | void>
}

type Saved = { list: Suffix[]; active: Record<string, ActiveSuffix> }

type Store = Saved & {
  /** The connected engine when it keeps the suffixes; `null`: this device's `localStorage`. */
  engine: SuffixEngine | null
  /** Engine mode: each watched session's `always` suffix id as its engine stores it (`null`: none). */
  always: Record<string, string | null>
  /** A new suffix; returns its id. */
  add(name: string, text: string): string
  update(id: string, patch: Partial<Omit<Suffix, 'id'>>): void
  /** Deleting a suffix turns it off in every chat. */
  remove(id: string): void
  activate(chat: string, id: string, scope: SuffixScope): void
  /** Turn off the chat's suffix (the chip's ×). */
  clear(chat: string): void
  /** The engine took a message of `chat`: a `once` suffix is spent. */
  sent(chat: string): void
  /** A draft became session `to`: its suffix (an `always` one; a `once` one if not yet spent) goes with it. */
  carry(from: string, to: string): void
  /** Engine mode: (re)read `chat`'s `always` suffix from its engine. No-op for a draft key or local mode. */
  watch(chat: string): Promise<void>
  /** Use `engine` for the list and `always` activations (migrating a local list once); `null` goes back to local. */
  connect(engine: SuffixEngine | null): Promise<void>
  /** Engine mode: reload the list and every watched session (the `agent.suffixes` topic). */
  refresh(): Promise<void>
}

export function load(): Saved {
  try {
    const s = JSON.parse(localStorage.getItem(KEY) ?? '{}') as Partial<Saved>
    return { list: Array.isArray(s.list) ? s.list : [], active: s.active ?? {} }
  } catch {
    return { list: [], active: {} }
  }
}

function save(s: Saved): void {
  try {
    localStorage.setItem(KEY, JSON.stringify({ list: s.list, active: s.active } satisfies Saved))
  } catch {
    // Storage full or disabled: the suffixes still live in the store for this run.
  }
}

function migrated(): boolean {
  try {
    return localStorage.getItem(MIGRATED) !== null
  } catch {
    return true
  }
}

const without = <T>(map: Record<string, T>, chat: string): Record<string, T> => {
  const { [chat]: _, ...rest } = map
  return rest
}

/** Session ids are uuids; a draft key (`''`, `panel:…`) is not a chat on any engine yet. */
const isSession = (chat: string): boolean => /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/i.test(chat)

const warn = (what: string) => (e: unknown) => console.warn(`prompt suffixes: ${what}`, e)

export const useSuffixes = create<Store>()((set, get) => {
  /** Persist `active` always, the list only while it is this device's (engine mode keeps the local copy as it was). */
  const put = (patch: Partial<Saved>) => {
    const next = { list: get().list, active: get().active, ...patch }
    save(get().engine ? { list: load().list, active: next.active } : next)
    set(next)
  }
  const setAlways = (chat: string, id: string | null) => {
    const engine = get().engine
    if (!engine) return
    set({ always: { ...get().always, [chat]: id } })
    void engine.set_session_suffix(chat, id).catch(warn('set_session_suffix'))
  }
  return {
    ...load(),
    engine: null,
    always: {},
    add(name, text) {
      const id = crypto.randomUUID()
      put({ list: [...get().list, { id, name, text }] })
      void get().engine?.put_suffix({ id, name, text }).catch(warn('put_suffix'))
      return id
    },
    update(id, patch) {
      put({ list: get().list.map((s) => (s.id === id ? { ...s, ...patch } : s)) })
      const s = get().list.find((x) => x.id === id)
      if (s) void get().engine?.put_suffix(s).catch(warn('put_suffix'))
    },
    remove(id) {
      put({
        list: get().list.filter((s) => s.id !== id),
        active: Object.fromEntries(Object.entries(get().active).filter(([, a]) => a.id !== id)),
      })
      const engine = get().engine
      if (!engine) return
      set({ always: Object.fromEntries(Object.entries(get().always).map(([k, v]) => [k, v === id ? null : v])) })
      void engine.delete_suffix(id).catch(warn('delete_suffix'))
    },
    activate(chat, id, scope) {
      if (get().engine && isSession(chat)) {
        if (scope === 'always') {
          put({ active: without(get().active, chat) })
          setAlways(chat, id)
          return
        }
        // "This message" replaces an "every message" suffix, as it does locally.
        if (get().always[chat]) setAlways(chat, null)
      }
      put({ active: { ...get().active, [chat]: { id, scope } } })
    },
    clear(chat) {
      if (chat in get().active) put({ active: without(get().active, chat) })
      if (get().always[chat]) setAlways(chat, null)
    },
    sent(chat) {
      if (get().active[chat]?.scope === 'once') put({ active: without(get().active, chat) })
    },
    carry(from, to) {
      const a = get().active[from]
      if (!a || from === to) return
      if (a.scope === 'always' && get().engine && isSession(to)) {
        put({ active: without(get().active, from) })
        setAlways(to, a.id)
        return
      }
      put({ active: { ...without(get().active, from), [to]: a } })
    },
    async watch(chat) {
      const engine = get().engine
      if (!engine || !isSession(chat)) return
      try {
        const id = await engine.session_suffix(chat)
        if (get().engine === engine) set({ always: { ...get().always, [chat]: id ?? null } })
      } catch (e) {
        warn('session_suffix')(e)
      }
    },
    async connect(engine) {
      if (get().engine === engine) return
      if (!engine) {
        set({ engine: null, always: {}, list: load().list })
        return
      }
      try {
        const local = load().list
        if (local.length > 0 && !migrated()) {
          for (const s of local) await engine.put_suffix(s)
          try {
            localStorage.setItem(MIGRATED, String(Date.now()))
          } catch {
            // Storage disabled: a later run uploads again, by id (no duplicates; deleted ids stay deleted).
          }
        }
        const list = await engine.list_suffixes()
        set({ engine, list, always: {} })
      } catch (e) {
        // An engine that advertised the methods but cannot answer (a legacy probe): stay on this device's list.
        warn('engine unavailable, using this device’s list')(e)
      }
    },
    async refresh() {
      const engine = get().engine
      if (!engine) return
      try {
        const list = await engine.list_suffixes()
        if (get().engine !== engine) return
        set({ list })
        await Promise.all(Object.keys(get().always).map((chat) => get().watch(chat)))
      } catch (e) {
        warn('list_suffixes')(e)
      }
    },
  }
})

// Two windows share the key: the other window's save replaces this copy (engine mode: only the draft/once state).
if (typeof window !== 'undefined')
  window.addEventListener('storage', (e) => {
    if (e.key !== KEY && e.key !== null) return
    const saved = load()
    useSuffixes.setState(useSuffixes.getState().engine ? { active: saved.active } : saved)
  })

/** What is on for a chat: this device's activation `local` (a draft's, a `once`, or local mode's), else the engine's `always` id. */
export function activeFor(local: ActiveSuffix | undefined, always: string | null | undefined): ActiveSuffix | undefined {
  return local ?? (always ? { id: always, scope: 'always' } : undefined)
}

/** The active suffix `on` with its saved text; `null` when none is on or it was deleted. */
export function activeSuffix(list: Suffix[], on: ActiveSuffix | undefined): (Suffix & { scope: SuffixScope }) | null {
  const suffix = on && list.find((x) => x.id === on.id)
  return suffix && on ? { ...suffix, scope: on.scope } : null
}

/** `body` with `suffix` appended after a blank line; a blank suffix changes nothing, an empty body is just the suffix. */
export function withSuffix(body: string, suffix: string | null | undefined): string {
  const tail = suffix?.trim()
  if (!tail) return body
  return body.trim() ? `${body.replace(/\s+$/, '')}\n\n${tail}` : tail
}
