// Composer state that outlives the composer, by session (`''` = new chat; `panel:<session>` = a note page's panel):
// a screen switch, a pane remount or a relaunch leaves the draft where it was. `useDrafts.by` is the typed-but-unsent
// text with the held chips (the composer renders it straight from the store, so no local copy can be lost on a
// remount); `useDrafts.edits` is an in-progress edit of a sent message (`MessageUser`'s bubble), kept apart from the
// composer draft so entering/cancelling an edit never touches it and a virtualised row scrolling out keeps the edit.
// `useDrafts.pick` is the new-chat draft's model/effort; `settings` holds its other per-chat choices.
// Both persist with its text. `opening` keeps a created chat when a pending setter fails, including across reloads.
// All persist to `localStorage` like `note-chat.ts`. `useSkillDraft` is the skills parked for the next send, in order
// and without duplicates (`/skill:<name>`, the `+` menu's Skills, or Use in the Skills panel), one `◇ name` chip each;
// the send is `use_skill(session, name, words)` for one, `use_skills(session, names, words)` for several (`sendSkills`).
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
import type { Api } from '@/rpc/bindings/client'
import type { CreateSessionOptions } from '@/rpc/bindings/method.sessionsync.input'
import type { SessionOpen } from "@/rpc/bindings/method.agent.output"
import type { PeerScope } from '@/rpc/bindings/method.agent.output'

const KEY = 'composer-drafts'

/** `unsent`: the client input id of a send not yet acked (in flight, or failed) — the retry reuses it. */
export type Draft = { text: string; pastes: Pastes; unsent?: string }
/** The bubble's state while `turn` of a session is being edited: its body and the reference chips not yet dropped. */
export type Edit = { turn: number; text: string; refs: AttachRef[] }

export const EMPTY: Draft = { text: '', pastes: new Map() }

/** The new-chat draft's model/effort; `null` = the engine's default (a draft cannot ask for the provider default). */
export type DraftPick = { model: string | null; effort: string | null }
export const NO_PICK: DraftPick = { model: null, effort: null }

/** New-chat options, spent only after every setter succeeds. Empty means inherit, not an explicit off. */
export type DraftSettings = {
  create?: Partial<CreateSessionOptions>
  approvals?: boolean | null
  communication?: boolean
  hidden?: Record<string, boolean>
  /** 219: tools the chat marks always-ask (`set_always_ask(id, name, true)` once it exists). */
  alwaysAsk?: string[]
  compactorModel?: string | null
  compactorEffort?: string | null
  /** The chat's own knowledge harvester `[model, effort]`; `null` or absent = the default. */
  knowledgeHarvester?: [string, string | null] | null
  viewBytes?: number
  scope?: PeerScope
  alias?: string | null
}
type Opening = { open: SessionOpen; engine: string | null; cwd: string | null }

/** `Pastes` is a `Map`: stored as its entries. */
type Saved = {
  drafts: Record<string, { text: string; pastes: [number, Paste][]; unsent?: string }>
  edits: Record<string, Edit>
  pick?: DraftPick
  settings?: DraftSettings
  opening?: Opening | null
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
  settings: DraftSettings
  opening: Opening | null
  setSettings(patch: Partial<DraftSettings>): void
  setCreate(patch: Partial<CreateSessionOptions>): void
  setHidden(names: string[], hidden: boolean): void
  /** 219: mark (`on`) or unmark `name` always-ask in the draft; the last unmark drops the setting (nothing pending). */
  setAlwaysAsk(name: string, on: boolean): void
  setOpening(opening: Opening | null): void
  spend(): void
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

type Persisted = Pick<Store, 'by' | 'edits' | 'pick' | 'settings' | 'opening'>

export function load(): Persisted {
  try {
    const s = JSON.parse(localStorage.getItem(KEY) ?? '{}') as Partial<Saved>
    const by: Record<string, Draft> = {}
    for (const [k, d] of Object.entries(s.drafts ?? {}))
      by[k] = { text: d.text, pastes: new Map(d.pastes), ...(d.unsent ? { unsent: d.unsent } : {}) }
    return { by, edits: s.edits ?? {}, pick: { ...NO_PICK, ...s.pick }, settings: s.settings ?? {}, opening: s.opening ?? null }
  } catch {
    return { by: {}, edits: {}, pick: NO_PICK, settings: {}, opening: null }
  }
}

function save({ by, edits, pick, settings, opening }: Persisted): void {
  const drafts: Saved['drafts'] = {}
  for (const [k, d] of Object.entries(by))
    if (d.text || d.pastes.size) drafts[k] = { text: d.text, pastes: [...d.pastes], ...(d.unsent ? { unsent: d.unsent } : {}) }
  try {
    localStorage.setItem(KEY, JSON.stringify({ drafts, edits, pick, settings, opening } satisfies Saved))
  } catch {
    // Storage full or disabled: the draft still lives in the store for this run.
  }
}

export const useDrafts = create<Store>()((set, get) => ({
  ...load(),
  setSettings(patch) {
    const settings = { ...get().settings, ...patch }
    save({ ...get(), settings })
    set({ settings })
  },
  setCreate(patch) {
    get().setSettings({ create: { ...get().settings.create, ...patch } })
  },
  setHidden(names, hidden) {
    get().setSettings({ hidden: { ...get().settings.hidden, ...Object.fromEntries(names.map((name) => [name, hidden])) } })
  },
  setAlwaysAsk(name, on) {
    const { alwaysAsk = [], ...others } = get().settings
    const names = on ? [...new Set([...alwaysAsk, name])].sort() : alwaysAsk.filter((n) => n !== name)
    const settings = names.length ? { ...others, alwaysAsk: names } : others
    save({ ...get(), settings })
    set({ settings })
  },
  setOpening(opening) {
    save({ ...get(), opening })
    set({ opening })
  },
  spend() {
    const next = { pick: NO_PICK, settings: {}, opening: null }
    save({ ...get(), ...next })
    set(next)
  },
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
  by: Record<string, string[]>
  /** Park `name` after the others; a skill already parked stays where it is. */
  add(session: string, name: string): void
  remove(session: string, name: string): void
  clear(session: string): void
}

export const useSkillDraft = create<SkillStore>()((set) => {
  const put = (session: string, f: (names: string[]) => string[]) =>
    set((s) => {
      const { [session]: was = [], ...rest } = s.by
      const names = f(was)
      return { by: names.length ? { ...rest, [session]: names } : rest }
    })
  return {
    by: {},
    add: (session, name) => put(session, (names) => (names.includes(name) ? names : [...names, name])),
    remove: (session, name) => put(session, (names) => names.filter((n) => n !== name)),
    clear: (session) => put(session, () => []),
  }
})

/** Send the parked `names` with the typed `words`: the server builds one input, every `<skill>` block in order, then
 * the words, with `files` attached as `send_input`'s. One skill keeps `use_skill`; several go in one `use_skills`.
 * Without files the optional parameter is left out: the same three-parameter call an older engine takes. */
export function sendSkills(api: Pick<Api, 'use_skill' | 'use_skills'>, session: string, names: readonly string[], words: string, files: readonly string[] = []): Promise<string> {
  if (!files.length) return names.length === 1 ? api.use_skill(session, names[0]!, words) : api.use_skills(session, [...names], words)
  const attached = [...files]
  return names.length === 1 ? api.use_skill(session, names[0]!, words, attached) : api.use_skills(session, [...names], words, attached)
}

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
  engine: useDrafts.getState().opening?.engine ?? null,
  cwd: useDrafts.getState().opening?.cwd ?? null,
  reuse: null,
  set: (patch) => set(patch),
}))

/** How long the first message to another machine waits for the chat's follow stream, so its first events are seen live. */
const LIVE_WAIT_MS = 2_000

/**
 * Open the session a draft's first action creates (send, Talk or skill). Any pending option selects
 * `create_session`; project, memory and other creation fields go in that call. Then approvals, messaging,
 * tool switches, always-ask marks, summary model/effort, view size and scope/alias are awaited in order. Only successful
 * application spends the pick/settings. A failed setter names the setting and leaves the draft and its
 * created chat intact for retry; no first input can run before this resolves.
 */
export async function openDraftSession(
  api: Api,
  options: CreateSessionOptions | null = null,
  input?: string,
): Promise<{ id: string; open: SessionOpen | null; pick: DraftPick }> {
  const draft = useDrafts.getState()
  const { pick, settings, opening } = draft
  const target = useDraftTarget.getState()
  const { reuse } = target
  const held = opening && opening.engine === target.engine && opening.cwd === target.cwd ? opening.open : null
  const again = held?.session ?? (reuse && reuse.input === input && reuse.engine === target.engine && reuse.cwd === target.cwd ? reuse.session : null)
  const pending = Object.keys(settings).length > 0
  const base = options ?? (pending || pick.model || pick.effort || target.engine || target.cwd ? { ...PLAIN_OPTIONS, ...pick } : null)
  const wanted = !again && base && { ...base, ...settings.create, ...(target.engine ? { engine: target.engine } : {}), ...(target.cwd ? { cwd: target.cwd } : {}) }
  const open = held ?? (wanted ? await api.create_session(wanted) : null)
  const id = again ?? (open ? open.session : await api.new_session())
  // Persist before applying anything: a rejected setter or a reload must not orphan this empty chat.
  if (pending && open) draft.setOpening({ open, engine: target.engine, cwd: target.cwd })
  const apply = async (name: string, call: () => Promise<unknown>) => {
    try {
      await call()
    } catch (cause) {
      throw new Error(`Could not apply ${name}: ${cause instanceof Error ? cause.message : String(cause)}`, { cause })
    }
  }
  // A retry may have edited the create-time choices while the empty chat was retained.
  if (again && settings.create?.project !== undefined) await apply('project', () => api.set_session_project(id, settings.create!.project ?? null))
  if (again && settings.create?.memory) await apply('memory', () => api.set_session_memory(id, settings.create!.memory!))
  if (held && pick.model) await apply('model', () => api.set_model(id, pick.model!, true))
  if (held) await apply('effort', () => api.set_effort(id, pick.effort, true))
  if (settings.approvals !== undefined) await apply('approvals', () => api.set_approvals(id, settings.approvals ?? true, settings.approvals === null))
  if (settings.communication !== undefined) await apply('messaging', () => api.set_communication(id, settings.communication!))
  for (const hidden of [false, true]) {
    const names = Object.entries(settings.hidden ?? {}).filter(([, value]) => value === hidden).map(([name]) => name)
    if (names.length) await apply('tools and MCP switches', () => api.set_mcp_hidden(id, names, hidden))
  }
  for (const name of settings.alwaysAsk ?? []) await apply('always ask', () => api.set_always_ask(id, name, true))
  // A model change can clear its effort, so the model must be installed first.
  if (settings.compactorModel !== undefined) await apply('summary model', () => api.set_session_compactor_model(id, settings.compactorModel!))
  if (settings.compactorEffort !== undefined) await apply('summary effort', () => api.set_session_compactor_effort(id, settings.compactorEffort!))
  if (settings.knowledgeHarvester) await apply('knowledge harvester', () => api.set_session_knowledge_harvester(id, settings.knowledgeHarvester![0], settings.knowledgeHarvester![1]))
  if (settings.viewBytes !== undefined) await apply('memory view size', () => api.set_session_view_bytes(id, settings.viewBytes!))
  if (settings.scope !== undefined) await apply('messaging scope and alias', () => api.set_peer_scope(id, settings.scope!, settings.alias ?? null))
  draft.spend()
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
