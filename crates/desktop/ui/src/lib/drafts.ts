// Composer state that outlives the composer, by session (`''` = new chat; `panel:<session>` = a note page's panel):
// a screen switch, a pane remount or a relaunch leaves the draft where it was. `useDrafts.by` is the typed-but-unsent
// text with the held chips (the composer renders it straight from the store, so no local copy can be lost on a
// remount); `useDrafts.edits` is an in-progress edit of a sent message (`MessageUser`'s bubble), kept apart from the
// composer draft so entering/cancelling an edit never touches it and a virtualised row scrolling out keeps the edit.
// `useDrafts.pick` is the new-chat draft's model/effort (148 R1), kept with its text for the same reasons.
// All persist to `localStorage` like `note-chat.ts`. `useSkillDraft` is a skill parked for the next send
// (`/skill:<name>` or Use in the Skills panel), shown as a `◇ name` chip and sent as `use_skill(session, name, words)`.
//
// A send never clears the draft before the engine acks it (70 SEND-NEVER-LOSES): `sending` marks the draft with the
// client input id the send carries, `acked` drops what was sent, and a failure leaves the marked draft where it is,
// so a reload mid-send or a dropped socket lands the text back in the box with the "Not sent" banner; the retry
// reuses the id, which the engine dedupes.

import { create } from 'zustand'
import type { AttachRef } from '@/lib/attach'
import type { Paste, Pastes } from '@/lib/pastes'
import { isNoSuchSession } from '@/lib/no-such-session'
import { untilLive, useRemoteStart } from '@/lib/remote-start'
import type { Api } from '@/rpc/api'
import type { CreateSessionOptions } from '@/rpc/bindings/CreateSessionOptions'
import type { SessionOpen } from '@/rpc/bindings/SessionOpen'

const KEY = 'composer-drafts'

/** `unsent`: the client input id of a send not yet acked (in flight, or failed) — the retry reuses it. */
export type Draft = { text: string; pastes: Pastes; unsent?: string }
/** The bubble's state while `turn` of a session is being edited: its body and the reference chips not yet dropped. */
export type Edit = { turn: number; text: string; refs: AttachRef[] }

export const EMPTY: Draft = { text: '', pastes: new Map() }

/** The new-chat draft's model/effort; `null` = the engine's default (a draft cannot ask for the provider default). */
export type DraftPick = { model: string | null; effort: string | null }
export const NO_PICK: DraftPick = { model: null, effort: null }

/** `Pastes` is a `Map`: stored as its entries. */
type Saved = {
  drafts: Record<string, { text: string; pastes: [number, Paste][]; unsent?: string }>
  edits: Record<string, Edit>
  pick?: DraftPick
}

type Store = {
  by: Record<string, Draft>
  edits: Record<string, Edit>
  /**
   * 148 R1/R2: the `''` draft's model/effort pick. It lives exactly as long as the draft text — remounts, route
   * round-trips and a relaunch keep it — and never touches the engine default: `openDraftSession` creates the session
   * with it, then `resetPick` spends it (the only path that consumes the draft).
   * ponytail: one pick for every draft pane; per-pane picks if split drafts ever need to differ.
   */
  pick: DraftPick
  set(key: string, draft: Draft): void
  setText(key: string, text: string): void
  setPastes(key: string, pastes: Pastes | ((held: Pastes) => Pastes)): void
  /** A send of the draft under `key` is in flight as client input `id`; the text stays until `acked`. */
  sending(key: string, id: string): void
  /** The engine took `text`: it leaves the box (anything typed after it during the flight stays). */
  acked(key: string, text: string): void
  /** Start editing `turn` of `session`, seeded with the message; any other edit in that session is dropped. */
  beginEdit(session: string, edit: Edit): void
  updateEdit(session: string, patch: Partial<Omit<Edit, 'turn'>>): void
  /** Cancel or send: the edit goes, the composer draft was never touched. */
  endEdit(session: string): void
  setPick(patch: Partial<DraftPick>): void
  resetPick(): void
}

type Persisted = Pick<Store, 'by' | 'edits' | 'pick'>

export function load(): Persisted {
  try {
    const s = JSON.parse(localStorage.getItem(KEY) ?? '{}') as Partial<Saved>
    const by: Record<string, Draft> = {}
    for (const [k, d] of Object.entries(s.drafts ?? {}))
      by[k] = { text: d.text, pastes: new Map(d.pastes), ...(d.unsent ? { unsent: d.unsent } : {}) }
    return { by, edits: s.edits ?? {}, pick: { ...NO_PICK, ...s.pick } }
  } catch {
    return { by: {}, edits: {}, pick: NO_PICK }
  }
}

function save({ by, edits, pick }: Persisted): void {
  const drafts: Saved['drafts'] = {}
  for (const [k, d] of Object.entries(by))
    if (d.text || d.pastes.size) drafts[k] = { text: d.text, pastes: [...d.pastes], ...(d.unsent ? { unsent: d.unsent } : {}) }
  try {
    localStorage.setItem(KEY, JSON.stringify({ drafts, edits, pick } satisfies Saved))
  } catch {
    // Storage full or disabled: the draft still lives in the store for this run.
  }
}

export const useDrafts = create<Store>()((set, get) => ({
  ...load(),
  set(key, draft) {
    // An emptied box has nothing left to retry.
    const { unsent, ...rest } = draft
    const by = { ...get().by, [key]: unsent && (draft.text || draft.pastes.size) ? draft : rest }
    save({ ...get(), by })
    set({ by })
  },
  setText(key, text) {
    get().set(key, { ...(get().by[key] ?? EMPTY), text })
  },
  setPastes(key, pastes) {
    const cur = get().by[key] ?? EMPTY
    get().set(key, { ...cur, pastes: typeof pastes === 'function' ? pastes(cur.pastes) : pastes })
  },
  sending(key, id) {
    get().set(key, { ...(get().by[key] ?? EMPTY), unsent: id })
  },
  acked(key, text) {
    const cur = get().by[key] ?? EMPTY
    get().set(
      key,
      cur.text.startsWith(text)
        ? { text: cur.text.slice(text.length), pastes: EMPTY.pastes }
        : { text: cur.text, pastes: cur.pastes },
    )
  },
  beginEdit(session, edit) {
    const edits = { ...get().edits, [session]: edit }
    save({ ...get(), edits })
    set({ edits })
  },
  updateEdit(session, patch) {
    const cur = get().edits[session]
    if (!cur) return
    get().beginEdit(session, { ...cur, ...patch })
  },
  endEdit(session) {
    const { [session]: _, ...edits } = get().edits
    save({ ...get(), edits })
    set({ edits })
  },
  setPick(patch) {
    const pick = { ...get().pick, ...patch }
    save({ ...get(), pick })
    set({ pick })
  },
  resetPick() {
    get().setPick(NO_PICK)
  },
}))

// Two windows on one chat (`lib/popout.ts`) share this key: a save in one replaces the other's copy, so the
// next save there carries every draft forward — last writer wins, per keystroke, never a stale whole map.
if (typeof window !== 'undefined')
  window.addEventListener('storage', (e) => {
    if (e.key === KEY || e.key === null) useDrafts.setState(load())
  })

type SkillStore = {
  by: Record<string, string>
  set(session: string, name: string | null): void
}

export const useSkillDraft = create<SkillStore>()((set) => ({
  by: {},
  set: (session, name) =>
    set((s) => {
      const { [session]: _, ...rest } = s.by
      return { by: name === null ? rest : { ...rest, [session]: name } }
    }),
}))

/** `create_session` options that give exactly what `new_session()` gives: every switch off, the engine's defaults. */
export const PLAIN_OPTIONS: CreateSessionOptions = {
  retention: 'durable',
  project: null,
  notebook: null,
  view: null,
  cwd: null,
  model: null,
  effort: null,
  parent_session: null,
  parent_engine: null,
  harness: null,
}

/** A chat created on `engine` (folder `cwd`) whose first message (client input id `input`) did not go: resending that
 * same message from the draft goes there. */
export type Reuse = { session: string; engine: string; cwd: string | null; input: string }

type DraftTarget = { engine: string | null; cwd: string | null; reuse: Reuse | null }

/** 183 §5.6 (W-10): the new chat's machine (mesh id) and folder there; `null` = this machine, its default folder. Not
 * persisted: a relaunch starts on this machine. Spent with the pick by `openDraftSession`. `reuse`: the empty chat a
 * failed first message left on that machine; its retry sends into it instead of leaving it behind for another. */
export const useDraftTarget = create<DraftTarget & { set(patch: Partial<DraftTarget>): void }>()((set) => ({
  engine: null,
  cwd: null,
  reuse: null,
  set: (patch) => set(patch),
}))

/** How long the first message to another machine waits for the chat's follow stream, so its first events are seen live. */
const LIVE_WAIT_MS = 2_000

/**
 * 148 R1: open the session a draft's first action creates (a send, Talk, `/<skill>`, a parked skill). `options`
 * (a New Chat switch on, built by the composer with the pick in it) or a bare model/effort pick goes through
 * `create_session`; otherwise `new_session()`. The pick is spent either way. `open` is the `create_session` reply
 * when it ran (a private session needs it); `pick` is what was spent, for seeding the transcript header.
 * 183 §5.6: a chosen machine or folder (`useDraftTarget`) also goes through `create_session`, with `engine` / `cwd`;
 * the retry of a failed first message (`input`, its client input id) to that same machine and folder opens its `reuse`
 * again instead (nothing is created).
 */
export async function openDraftSession(
  api: Api,
  options: CreateSessionOptions | null = null,
  input?: string,
): Promise<{ id: string; open: SessionOpen | null; pick: DraftPick }> {
  const { pick, resetPick } = useDrafts.getState()
  const target = useDraftTarget.getState()
  const { reuse } = target
  const again = reuse && reuse.input === input && reuse.engine === target.engine && reuse.cwd === target.cwd ? reuse.session : null
  const base = options ?? (pick.model || pick.effort || target.engine || target.cwd ? { ...PLAIN_OPTIONS, ...pick } : null)
  const wanted = !again && base && { ...base, ...(target.engine ? { engine: target.engine } : {}), ...(target.cwd ? { cwd: target.cwd } : {}) }
  const open = wanted ? await api.create_session(wanted) : null
  const id = again ?? (open ? open.session : await api.new_session())
  resetPick()
  target.set({ engine: null, cwd: null, reuse: null })
  return { id, open, pick }
}

/**
 * The first message (client input id `input`) of a chat just created on another machine: `open` the chat at once (the
 * caller has shown the message pending), wait for its follow stream (≤ `LIVE_WAIT_MS`: the stream has no replay, so a
 * fast first turn would otherwise only arrive with the rows), then `send` it. On a failure the user is taken `back` to
 * the new-chat draft — its text was never acked, so it is still in the box — with the machine and folder picked again,
 * and the error names the machine. The chat just created is kept for the retry (`reuse`): resending that message goes
 * into it with the same client input id, which the engine dedupes, so no empty chat is left behind and nothing is sent
 * twice. Never deleted: the failed send may have landed. Not kept when the machine itself says it has no such session.
 */
export async function sendFirstElsewhere(o: {
  session: string
  machine: string
  target: { engine: string; cwd: string | null }
  input: string
  open(): Promise<unknown>
  back(): Promise<unknown>
  send(): Promise<unknown>
}): Promise<void> {
  useRemoteStart.getState().begin(o.session, o.machine)
  await o.open()
  await untilLive(o.session, LIVE_WAIT_MS)
  try {
    await o.send()
  } catch (e) {
    useRemoteStart.getState().forget(o.session)
    useDraftTarget.getState().set({ ...o.target, reuse: isNoSuchSession(e) ? null : { session: o.session, ...o.target, input: o.input } })
    await o.back()
    throw new Error(`${o.machine} did not take the first message (${e instanceof Error ? e.message : String(e)}). It is back in the box: send again to retry.`, { cause: e })
  }
}

/**
 * A New chat draft's first message (client input id `input`): `openDraftSession`, then `created` seeds the chat with
 * the message, then `send`. On another machine (the picked engine has a `machine` name) with `open`/`back` given, the
 * chat opens at once and a failure returns to the draft (`sendFirstElsewhere`); a failed create names the machine.
 * Otherwise the caller opens the chat after the send.
 */
export async function sendFirst(o: {
  api: Api
  options: CreateSessionOptions | null
  input: string
  /** The picked engine's name for errors and "Starting on …"; `null` for this machine. */
  machine(engine: string): string | null
  created(id: string, open: SessionOpen | null, pick: DraftPick): Promise<void>
  send(id: string): Promise<unknown>
  open?: (id: string) => Promise<unknown>
  back?: (id: string) => Promise<unknown>
}): Promise<void> {
  const { engine, cwd } = useDraftTarget.getState()
  const machine = engine ? o.machine(engine) : null
  const { id, open, pick } = await openDraftSession(o.api, o.options, o.input).catch((e: unknown) => {
    throw machine ? new Error(`Could not start the chat on ${machine}: ${e instanceof Error ? e.message : String(e)}`, { cause: e }) : e
  })
  await o.created(id, open, pick)
  const { open: show, back } = o
  if (engine && machine && show && back)
    await sendFirstElsewhere({ session: id, machine, target: { engine, cwd }, input: o.input, open: () => show(id), back: () => back(id), send: () => o.send(id) })
  else await o.send(id)
}
