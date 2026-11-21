// #3 Prompt suffixes: named blocks of text the user saves once (Settings › Prompt suffixes) and turns on in the
// composer's `+` menu for one chat — for the next message only (`once`) or for every message until turned off
// (`always`). A send appends the active suffix's text to the outgoing message after a blank line, so it is part of
// the user's message in the transcript. Slash and skill commands never get it.
// `active` is keyed by chat: the session id, or the composer's draft key (`''`, `panel:`) for a chat not created
// yet, which `carry` moves onto the created session. `localStorage` + zustand like `note-chat.ts`.

import { create } from 'zustand'

const KEY = 'prompt-suffixes'

export type Suffix = { id: string; name: string; text: string }
/** `once`: the next send only; `always`: every send in that chat until turned off. */
export type SuffixScope = 'once' | 'always'
export type ActiveSuffix = { id: string; scope: SuffixScope }

type Saved = { list: Suffix[]; active: Record<string, ActiveSuffix> }

type Store = Saved & {
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

const without = (active: Record<string, ActiveSuffix>, chat: string): Record<string, ActiveSuffix> => {
  const { [chat]: _, ...rest } = active
  return rest
}

export const useSuffixes = create<Store>()((set, get) => {
  const put = (patch: Partial<Saved>) => {
    const next = { list: get().list, active: get().active, ...patch }
    save(next)
    set(next)
  }
  return {
    ...load(),
    add(name, text) {
      const id = crypto.randomUUID()
      put({ list: [...get().list, { id, name, text }] })
      return id
    },
    update(id, patch) {
      put({ list: get().list.map((s) => (s.id === id ? { ...s, ...patch } : s)) })
    },
    remove(id) {
      put({
        list: get().list.filter((s) => s.id !== id),
        active: Object.fromEntries(Object.entries(get().active).filter(([, a]) => a.id !== id)),
      })
    },
    activate(chat, id, scope) {
      put({ active: { ...get().active, [chat]: { id, scope } } })
    },
    clear(chat) {
      if (chat in get().active) put({ active: without(get().active, chat) })
    },
    sent(chat) {
      if (get().active[chat]?.scope === 'once') put({ active: without(get().active, chat) })
    },
    carry(from, to) {
      const a = get().active[from]
      if (a && from !== to) put({ active: { ...without(get().active, from), [to]: a } })
    },
  }
})

// Two windows share the key: the other window's save replaces this copy.
if (typeof window !== 'undefined')
  window.addEventListener('storage', (e) => {
    if (e.key === KEY || e.key === null) useSuffixes.setState(load())
  })

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
