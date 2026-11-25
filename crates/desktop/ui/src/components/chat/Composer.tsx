import { useQuerySupport } from '@/rpc/capabilities'
import { supports, UnsupportedCapability } from '@/rpc/discovery'
import { privateRequirements, endlessRequirements } from './requirements'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { open as pickFiles } from '@tauri-apps/plugin-dialog'
import {
  AudioLines,
  ChevronRight,
  CircleAlert,
  FileUp,
  Loader2,
  MessageSquarePlus,
  Paperclip,
  Plus,
  Square,
  StickyNote,
  X } from 'lucide-react'
import {
  type ClipboardEvent,
  type DragEvent,
  Fragment,
  type KeyboardEvent,
  type ReactNode,
  type RefObject,
  type SyntheticEvent,
  useEffect,
  useMemo,
  useRef,
  useState,
} from 'react'
import { notify as toast } from '@/lib/notify'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Textarea } from '@/components/ui/textarea'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { DictionarySuggestion } from '@/components/ui/DictionarySuggestion'
import * as dictation from '@/lib/dictation'
import { extFor, putAttachment, seedFile } from '@/lib/files'
import { usePanes } from '@/lib/panes'
import { type AttachRef, type AttachRequest, DRAG_TYPE, atTrigger, compose, toRef } from '@/lib/attach'
import { loadRecents, pushRecents, saveRecents, stripAt } from '@/lib/attach-picker'
import { attach, attachRef, drop, expand, type Pastes, reconcileShelved, type Shelf, tokenize, untoken } from '@/lib/pastes'
import { pasteText } from '@/lib/paste-markdown'
import { applyText } from '@/lib/text-edit'
import { isDesktop, touch } from '@/lib/platform'
import { engineLabel, ownerUnreachable, parkIfOffline, retryNotOwner, routed, sessionDraining, useOwnerOffline } from '@/lib/owner'
import { VIEW, isPrivate, usePrivate } from '@/lib/private'
import { queryKeys, useApi } from '@/lib/rpc'
import { useMesh } from '@/components/tasks/queries'
import type { CreateSessionOptions } from '@/rpc/bindings/method.sessionsync.input'
import { EMPTY, PLAIN_OPTIONS, sendFirst, useDraftTarget, useDrafts, useSkillDraft } from '@/lib/drafts'
import { sendParked } from '@/lib/send-parked'
import { type Suffix, activeFor, activeSuffix, useSuffixes, withSuffix } from '@/lib/suffixes'
import { YOUNG_MS, useRemoteStart } from '@/lib/remote-start'
import { DEBOUNCE_MS as PATH_DEBOUNCE_MS, type PathPopup, accept as acceptPath, pathKey, pathTrigger, step as stepPath } from '@/lib/path-complete'
import { useIsMobile } from '@/hooks/use-mobile'
import { interrupt, phase, press } from '@/lib/composer-state'
import { shouldRefocus } from '@/lib/composer-focus'
import * as speech from '@/lib/speech'
import * as talk from '@/lib/talk'
import { label as chordLabel, matches as chord } from '@/lib/chord'
import { useShortcuts } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'
import { SendMark } from '@/components/Mark'
import { noteText } from '@/lib/voice-note'
import { stripRows } from '@/lib/session-tree'
import { type Act as StopAct, IDLE as STOP_IDLE, type Stop as StopState, progress as stopProgress, step as stepStop } from '@/lib/stop-gesture'
import type { CancelScope } from '@/rpc/bindings/method.agent.input'
import { LOCAL, useEngines } from '@/link/engines'
import { useConnection } from '@/store/connection'
import { useSessionField, useSessions } from '@/store/sessions'
import { createNote } from '@/components/notes/create'
import { useNotes } from '@/components/notes/useNotes'
import { AttachPicker } from './AttachPicker'
import { PillRow, SkillPicker, SkillsEntry } from './SkillPicker'
import { SuffixList, SuffixScopes } from './SuffixPicker'
import { CommunicationNotice, useCommunication } from './CommunicationNotice'
import { useNextPick } from './NextSteps'
import { ContextPicker } from './ContextPicker'
import { MicButton } from './MicButton'
import { useNoteRecorder } from './use-note-recorder'
import { StopButton } from './StopButton'
import { CancelHint, DeleteNote, RecordingMeta, type Stage } from './VoiceHold'
import { HeldStrip } from './HeldStrip'
import { type Uploaded, uploadWait, useUploads } from './uploads'
import { McpMenu } from './McpMenu'
import { ApprovalShield, ComposerMeta, ContextButton, useComposerMeta } from './ComposerMeta'
import { ComposerModel } from './ModelMenu'
import { OwnerOfflineCard } from './OwnerOfflineCard'
import { RecordingBox, VoiceWave, useVoiceSource } from '@/components/voice/VoiceWave'
import { tapOrLevels } from '@/lib/visual-source'
import { QueuedChips, withdrawInto } from './QueuedChips'
import { COMMANDS, type Ctx, FRESH, type HarnessMenu, type SlashItem, draftSkill, isCommand, run, sendRaw, slashMenu } from './slash'
import { PluginCommandCard } from '@/components/plugins/PluginSurface'
import { usePluginCommands } from '@/lib/plugins'

/** New Chat options (51 §8.3, 137 §11.3): `private` selects memory-only retention; `project` is root metadata; `endless` the summary-tree memory. */
export type DraftCreate = { private: boolean; project: string | null; endless: boolean }
/** The draft with every switch off: what `new_session()` would create. */
export const PLAIN_DRAFT: DraftCreate = { private: false, project: null, endless: false }

/**
 * The `create_session` options for a draft; `memory` is sent only when the ∞ switch is on (the engine's default
 * otherwise). `pick` is the draft's model/effort (148 R1): this session's only, `null` = the engine's default.
 */
export function createOptions(draft: DraftCreate, pick: { model: string | null; effort: string | null } = { model: null, effort: null }): CreateSessionOptions {
  return {
    ...PLAIN_OPTIONS,
    retention: draft.private ? 'private' : 'durable',
    project: draft.project,
    view: draft.private ? VIEW : null,
    model: pick.model,
    effort: pick.effort,
    memory: draft.endless ? 'endless' : undefined,
  }
}

/** A machine name the box never breaks inside: no-break spaces, and a word joiner after each hyphen (`cloud-1`). */
const unbroken = (name: string): string => name.replace(/ /g, '\u00a0').replace(/-/g, '-\u2060')

/** The box width (px) under which the placeholder takes its short form. 215 C2: measured on the whole box now that the
 * text runs its full width (the buttons sit on the toolbar row underneath). */
const NARROW_BOX_PX = 480

/** Whether `ref`'s element is narrower than `max` px, live. */
function useNarrow(ref: RefObject<HTMLElement | null>, max: number): boolean {
  const [narrow, setNarrow] = useState(false)
  useEffect(() => {
    const el = ref.current
    if (!el || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(([e]) => e && setNarrow(e.contentRect.width < max))
    ro.observe(el)
    return () => ro.disconnect()
  }, [ref, max])
  return narrow
}

/** `ref`'s element's content width (px), live; `undefined` until measured (or with no ResizeObserver). */
function useWidth(ref: RefObject<HTMLElement | null>): number | undefined {
  const [width, setWidth] = useState<number | undefined>(undefined)
  useEffect(() => {
    const el = ref.current
    if (!el || typeof ResizeObserver === 'undefined') return
    const ro = new ResizeObserver(([e]) => e && setWidth(Math.floor(e.contentRect.width)))
    ro.observe(el)
    return () => ro.disconnect()
  }, [ref])
  return width
}

/** The box's placeholder. An exported session's human input never joins a peer task; it steers a human turn or waits
 * behind a task (50 §6.3). 183 §12.1 / mock panel 3: a chat owned elsewhere names the machine it runs on, a steer too;
 * `short` (a narrow window or a touch screen) drops the `Message — ` lead, and the name itself never breaks. */
export function placeholderFor(o: { readOnly?: boolean; running: boolean; exports: boolean; skill?: string; runsOn?: string; short?: boolean }): string {
  const on = o.runsOn && unbroken(o.runsOn)
  const remote = on && (o.short ? `Runs on ${on}` : `Message — runs on ${on}`)
  if (o.readOnly) return 'Talking — speak, or stop the talk to type'
  if (o.running) return o.exports ? 'Send privately…' : remote || 'Send to steer…'
  if (o.skill) return `Words for ${o.skill} (optional)`
  return remote || '…'
}

const NONE = EMPTY.pastes
const NO_SKILLS: readonly string[] = []
const IMAGE = /\.(png|jpe?g|gif|webp|bmp)$/i
/**
 * Seed the next mount of `session`'s composer (`null` = the new chat at `/`; `panel` = a note page's panel) with `text`
 * and a held chip per reference in `refs` (59 §5.4 "Ask theuth…", 66 §7.8 "Attach to chat…").
 */
export function prefillDraft(session: string | null, text: string, refs: AttachRef[] = [], panel = false): void {
  useDrafts.getState().set(`${panel ? 'panel:' : ''}${session ?? ''}`, { text, pastes: refs.reduce((held, r) => attachRef(r, held), NONE) })
}
/** Append `text` to `session`'s draft, keeping what is typed and held (73 §6.8 "Ask about this"); `head` is prepended only when the draft is empty. */
export function appendDraft(session: string, text: string, head = ''): void {
  const cur = useDrafts.getState().by[session] ?? EMPTY
  useDrafts.getState().set(session, { ...cur, text: cur.text ? `${cur.text}${text}` : `${head}${text}` })
}

/** 153 §4.2 stage 4: the slash commands dictation may write — theuth's, and the harness's with their slash (`skill:x` → `/skill:x`). */
export function dictationCommands(harness: readonly { name: string }[] | null | undefined): string[] {
  return [...COMMANDS.map((c) => c.name), ...(harness ?? []).map((c) => (c.name.startsWith('/') ? c.name : `/${c.name}`))]
}

/**
 * 153 §7.4 "send it": `auto` is the landed value waiting to be sent; it fires once the box's `text` is that value and
 * is cleared in the same step, so one spoken send is one `submit()`. A dictation that only armed the phrase on a
 * partial lands with `send: false` (`lib/dictation.finish`) and never sets `auto`.
 */
export function spokenSend(auto: string | null, text: string): { fire: boolean; auto: string | null } {
  return auto !== null && text === auto ? { fire: true, auto: null } : { fire: false, auto }
}

/** A send's ack is one round trip ("returns once queued"): longer means a socket the OS never closed (70 SEND-NEVER-LOSES). */
const SEND_TIMEOUT_MS = 15_000
class SendTimeout extends Error {
  constructor() {
    super('Not sent: no reply from the engine')
  }
}
/** `p`, or `SendTimeout` after `SEND_TIMEOUT_MS`; the caller probes the socket so a dead one reconnects. */
async function deadline<T>(p: Promise<T>): Promise<T> {
  let timer: number | undefined
  const late = new Promise<never>((_, reject) => {
    timer = window.setTimeout(() => reject(new SendTimeout()), SEND_TIMEOUT_MS)
  })
  try {
    return await Promise.race([p, late])
  } finally {
    clearTimeout(timer)
  }
}
/** The edit box, textarea and its read-only stand-in while dictating (54 §7). 215 C2: a slim inset so the text lines up
 * with the toolbar's first button; one line tall until it grows. */
const BOX =
  'min-h-8 resize-none border-0 bg-transparent px-1 py-1 text-[17px] leading-6 shadow-none focus-visible:ring-0 md:text-[17px] dark:bg-transparent'
/**
 * The bottom bar's narrow-width fit, from the box's measured content width (`useNarrow`, a ResizeObserver) rather than a
 * CSS container query: `container-type` puts the bar in a containment context, and iOS WebKit has left content inside
 * one unpainted. Under 300 px the gaps tighten and the round buttons go 36 → 32 px (a running talk plus Steer and Stop
 * still need it in a 280 px pane); nothing is ever hidden for room.
 */
export const BAR_TIGHT_PX = 300
export const barFit = (tight: boolean): string => cn(tight && 'gap-0.5 [&_button.size-9]:size-8')
/** A press held this long on Send/Steer is the interrupt gesture (items 7/8): fingers have no ⌘↩. */
const LONG_PRESS = 500

/** Tap → `onPress`; held ≥ `LONG_PRESS` → `onLong`, and the click that follows the release is swallowed. */
function useLongPress(onPress: () => void, onLong: () => void) {
  const timer = useRef<number | undefined>(undefined)
  const fired = useRef(false)
  const clear = () => clearTimeout(timer.current)
  return {
    onPointerDown: () => {
      fired.current = false
      clear()
      timer.current = window.setTimeout(() => {
        fired.current = true
        onLong()
      }, LONG_PRESS)
    },
    onPointerUp: clear,
    onPointerLeave: clear,
    onPointerCancel: clear,
    onClick: () => {
      if (fired.current) fired.current = false
      else onPress()
    },
    // No callout / context menu under a held finger.
    onContextMenu: (e: SyntheticEvent) => e.preventDefault(),
  }
}

/** The slash popup: commands matching the typed word, one highlighted. D15: in an ACP session each row names its source (theuth / the harness). */
function SlashList({
  items,
  selected,
  onPick,
}: {
  items: readonly SlashItem[]
  selected: number
  onPick(item: SlashItem): void
}) {
  return (
    <div className="absolute bottom-full left-0 mb-2 max-h-72 w-full overflow-y-auto rounded-xl border bg-popover p-1 text-sm shadow-md">
      {items.map((c, i) => (
        <button
          type="button"
          key={`${c.source ?? ''}:${c.name}`}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => onPick(c)}
          className={cn(
            'flex w-full items-baseline gap-2 rounded-lg px-3 py-2 text-left',
            i === selected && 'bg-accent',
          )}
        >
          <span className="font-mono">{c.name}</span>
          {c.hint && <span className="font-mono text-fg-tertiary">{c.hint}</span>}
          <span className="ml-auto truncate text-fg-secondary">{c.desc}</span>
          {c.source && <span className="shrink-0 rounded-sm border border-border-subtle px-1 text-[10px] leading-4 text-fg-tertiary">{c.source}</span>}
        </button>
      ))}
    </div>
  )
}

/** PATH-AC: the path list anchored at the token; on a phone a chip row above the box. `…` when the engine cut the listing. */
function PathList({ popup, mobile, onPick }: { popup: PathPopup; mobile: boolean; onPick(i: number): void }) {
  return (
    <div
      className={cn(
        'absolute bottom-full left-0 mb-2 w-full rounded-xl border bg-popover p-1 font-mono text-sm shadow-md',
        mobile ? 'flex gap-1 overflow-x-auto' : 'max-h-72 overflow-y-auto',
      )}
    >
      {popup.items.map((c, i) => (
        <button
          type="button"
          key={c.insert}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => onPick(i)}
          className={cn(
            'rounded-lg px-3 py-2 text-left',
            mobile ? 'shrink-0 whitespace-nowrap border' : 'flex w-full',
            i === popup.selected && 'bg-accent',
            c.is_dir && 'text-fg-secondary',
          )}
        >
          {c.display}
        </button>
      ))}
      {popup.truncated && <span className="px-3 py-2 text-fg-tertiary">…</span>}
    </div>
  )
}

/**
 * ChatGPT's composer: `+` attach on the left, send / stop on the right, Enter
 * sends (mid-turn: queues a steer), ⌘/Ctrl+Enter interjects (cancels the turn
 * so the message runs now), Shift+Enter breaks a line. A large paste is held as a chip above the box (`lib/pastes`,
 * `HeldStrip`) and its `⟦paste n⟧` token lands at the caret; submit puts the block back where the token stands, so
 * several blocks interleave with typed words in order. Deleting a token drops its chip, a chip's ✕ its token. Attachments
 * and documents are chips without tokens. `/` at column 0 opens
 * the command list; `/skill:<name>` parks a skill on the next send; a built-in or known `/<skill>` runs now
 * (`isCommand`); any other `/x` — a path, a typo — is message text.
 */
export function Composer({
  session,
  pane = session,
  draftCreate,
  pinned,
  sessionTitle,
  onCreated,
  onUncreated,
  resolveSession,
  onSent,
  compact = false,
  readOnly = false,
  strip,
  ownModel,
  lead,
  deliver,
  draftScope,
  carries = false,
}: {
  session: string | null
  /** The split pane this composer sits in when it is not `session`'s (a draft pane's id); `null` = not tiled, always addressed. */
  pane?: string | null
  draftCreate?: DraftCreate
  /** 59 §7.3 chat under a note: this document's marker line leads every message (not removable here). */
  pinned?: AttachRef
  /** Title of the session the first message creates (a note's chat: the note title). */
  sessionTitle?: string
  onCreated?: (session: string) => void
  /** 183 §5.6: with `onCreated`, the first message of a chat just created on another machine did not go: put this
   * host back on its draft (the text is still there). Without it such a host opens the chat only after the send. */
  onUncreated?: (session: string) => void
  /** 128 R6: with `session` null, the first send goes to the session this resolves (theuth's desk) instead of a new one; the default navigation to `/c/$session` follows. */
  resolveSession?: () => Promise<string>
  /** A bound `session` took the message (steer or send_input): the host may follow it. */
  onSent?: (session: string) => void
  /** Tighter box for a panel. */
  compact?: boolean
  /** 76 §0.12: the talk view's transcript — the box shows but takes nothing; speech is the input. */
  readOnly?: boolean
  /** 171F B7: the kernel strip, drawn on top of the input box (below the queued chips). */
  strip?: ReactNode
  /** 215 C2: forwarded as `own` to the toolbar's model picker (157 D1: the page's own pane, a notebook's chat column). */
  ownModel?: boolean
  /** 218: buttons at the toolbar's left end, before Add (the whiteboard's tools button and session picker). */
  lead?: ReactNode
  /** 218: the host sends a plain message itself (the whiteboard's send: images, context, a first send's session) and
   * answers the session it went to; a new session it made is not navigated to (`onSent` gets it). Commands, `/skill:`
   * and parked skills keep their own paths. */
  deliver?: (m: { text: string; files: string[]; clientId: string }) => Promise<string>
  /** The draft key's prefix (default: `panel:` with `onCreated`, else none): a host that keeps its own drafts. */
  draftScope?: string
  /** 218: the host sends something besides the words (the whiteboard's fresh marks): an empty box can send. */
  carries?: boolean
}) {
  const client = useConnection(s => s.client)
  const canQuery1 = useQuerySupport(["list_models"])
  const canQuery1_2 = useQuerySupport(["vision_model"])
  const canQuery1_3 = useQuerySupport(["list_skills"])
  const canQuery1_4 = useQuerySupport(["list_sessions"])
  const api = useApi()
  const active = useEngines((s) => s.active)
  // Host paths (native drop / dialog) only mean something to the daemon on this machine.
  const hostFiles = isDesktop() && active === LOCAL
  const fileInput = useRef<HTMLInputElement>(null)
  const navigate = useNavigate()
  const turnRunning = useSessionField(session ?? '', 'running') ?? false
  const activity = useSessionField(session ?? '', 'activity')
  // A `/compact` mini-turn owns the session without a `TurnStarted` (07 §8.6): a plain send
  // must queue (steer, shown as a chip), and only the explicit stop/interject may cancel it.
  const running = turnRunning || activity === 'compacting'
  const queued = useSessionField(session ?? '', 'queued')
  const keys = useShortcuts((s) => s.chords)
  const model = useSessionField(session ?? '', 'model')
  const parent = useSessionField(session ?? '', 'parent')
  /** A "Next:" pick for this session (the transcript's last item, `useNextSteps`), sent here as typed. */
  const nextPick = useNextPick((s) => (session ? s.by[session] : undefined))
  const agent = useSessionField(session ?? '', 'agent')
  const harnessCommands = useSessionField(session ?? '', 'harnessCommands')
  const defaultModel = useConnection((s) => s.info?.default_model)
  // 148 R1: a new session starts on the draft's pick, else the engine's defaults (a draft pick never sets those).
  const defaultEffort = useConnection((s) => s.info?.default_effort)
  const { data: models } = useQuery({ enabled: canQuery1,
    queryKey: queryKeys.models,
    queryFn: () => api.list_models(),
  })
  // 52 §3.2: what happens to an image the main model cannot see; the server stays authoritative when this is stale.
  const { data: visionModel } = useQuery({ enabled: canQuery1_2,
    queryKey: queryKeys.visionModel,
    queryFn: () => api.vision_model(),
  })
  // The skill index: which `/<word>` sends are skills (`isCommand`); the popup itself lists only `COMMANDS`.
  const { data: skillIndex } = useQuery({ enabled: canQuery1_3,
    queryKey: queryKeys.skills,
    queryFn: () => api.list_skills(),
  })
  // A panel's composer (`onCreated`) keeps its own draft apart from the chat page's for the same session.
  // The draft is the store's (`lib/drafts`): durable across remounts, pane switches and a relaunch.
  const draftKey = `${draftScope ?? (onCreated ? 'panel:' : '')}${session ?? ''}`
  const { text, pastes, unsent } = useDrafts((s) => s.by[draftKey] ?? EMPTY)
  const setText = (t: string) => useDrafts.getState().setText(draftKey, t)
  const setPastes = (p: Pastes | ((held: Pastes) => Pastes)) => useDrafts.getState().setPastes(draftKey, p)
  /** Skills parked for the next send (`lib/drafts` `useSkillDraft`), in order: the send becomes `sendParked`. */
  const skills = useSkillDraft((s) => (session ? s.by[session] : undefined)) ?? NO_SKILLS
  /** `+` › Skills: park the pick here; a new chat is created and opened the way `/skill:` does (`draftSkill`), the
   * typed words and held chips moving with it. The menu closes; reopening it adds the next skill. */
  const pickSkill = (name: string) => {
    setMenu(false)
    setSkillStep(false)
    if (session) {
      useSkillDraft.getState().add(session, name)
      ref.current?.focus()
      return
    }
    const held = pastes
    draftSkill(api, navigate, null, name, text, suffixChat).then(
      (id) => {
        useDrafts.getState().setPastes(id, held)
        reset()
      },
      (e: unknown) => toast.error(e instanceof Error ? e.message : String(e)),
    )
  }
  /** #3: the chat's prompt suffix (`lib/suffixes`), keyed by session — or by the draft until the chat exists. An
   * "every message" suffix of a session comes from its engine when that engine keeps suffixes (`watch`). */
  const suffixChat = session ?? draftKey
  const suffixes = useSuffixes((s) => s.list)
  const suffixLocal = useSuffixes((s) => s.active[suffixChat])
  const suffixAlways = useSuffixes((s) => s.always[suffixChat])
  const suffixEngine = useSuffixes((s) => s.engine)
  useEffect(() => {
    void useSuffixes.getState().watch(suffixChat)
  }, [suffixChat, suffixEngine])
  const suffix = activeSuffix(suffixes, activeFor(suffixLocal, suffixAlways))
  const [selected, setSelected] = useState(0)
  /** D15: a harness command picked from the menu that takes arguments; the next send goes to the harness verbatim. */
  const [rawCmd, setRawCmd] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  /** The send button's eclipse: `sent` counts sends (each plays it once); `playing` keeps the button 1.4 s before Stop takes its place. */
  const [sent, setSent] = useState(0)
  const [playing, setPlaying] = useState(false)
  /** 68 §5 / 183 §4.4: a routed call the owner could not take (this send, a ⋯ setter, a stop…); the card above the
   * composer offers to take the session here, then runs it. */
  const offline = useOwnerOffline((s) => (session ? s.by[session] : undefined))
  /** 139 §9.3: the one automatic retry a `SessionDraining` refusal gets; reset by the next success. */
  const retried = useRef(false)
  /** 183 §13 row 2: the one automatic resend after a `NotOwner` refusal. */
  const ownerRetried = useRef(false)
  const { data: mesh } = useMesh()
  // 183 §12.1: a chat owned elsewhere says where it runs; `list_sessions` is the sidebar's warm cache.
  const { data: rows } = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions(), enabled: canQuery1_4 && (!!session) })
  const row = session ? rows?.find((r) => r.id === session) : undefined
  const runsOn = row && !row.here ? engineLabel(row.owner, mesh?.peers) : undefined
  // 183 §5.6 / §13 row 11: a chat just created on another machine holds the next send until that machine is heard
  // from (its first event) or its row lists here; the engine forwards by id meanwhile, so this is the fallback.
  const start = useRemoteStart((s) => (session ? s.by[session] : undefined))
  // Held at most `YOUNG_MS`; the row listing here ends it (as the first event does, `dispatch`).
  const [, tick] = useState(0)
  useEffect(() => {
    if (!start || !session) return
    if (row) {
      useRemoteStart.getState().forget(session)
      return
    }
    const t = setTimeout(() => tick((n) => n + 1), Math.max(0, start.at + YOUNG_MS - Date.now()))
    return () => clearTimeout(t)
  }, [start, row, session])
  const starting = start && !row && Date.now() - start.at < YOUNG_MS ? start.machine : null
  // The new-chat draft while its `create_session` runs on the picked machine (up to the engine's replication wait).
  const pickedEngine = useDraftTarget((s) => (session ? null : s.engine))
  const creatingOn = busy && pickedEngine && pickedEngine !== mesh?.self_id ? engineLabel(pickedEngine, mesh?.peers) : null
  const voice = speech.useVoice()
  // 45 §7.2 / 89 VOICE-HOLD: the voice-note recorder (shared with the whiteboard pill, 218); the note is transcribed and sent.
  const { recording, source: noteSource, record, stop: stopRecording, dropWave: dropNoteWave } = useNoteRecorder((blob) => void sendNote(blob))
  const [secs, setSecs] = useState(0)
  /** 89 VOICE-HOLD: what the mic gesture wants the row to show (held: dot + timer + hint; locked: Delete … Send). */
  const [stage, setStage] = useState<Stage>('idle')
  /** "‹ slide to cancel" in the text region; `MicButton` moves it with the finger. */
  const hint = useRef<HTMLSpanElement>(null)
  // 76 §0.12: one talk per daemon; the Talk button names (and opens) the session that holds it.
  const talking = talk.useTalk()
  const talkElsewhere = talking.phase !== 'off' && talking.session !== null && talking.session !== session
  /** B3: the talk runs on this very session and this is the chat view of it (the talk view's own composer is read-only). */
  const talkHere = talking.phase !== 'off' && session !== null && talking.session === session && !readOnly
  const talkTitle = useSessionField(talking.session ?? '', 'title')
  /** Uploading + transcribing a finished note. */
  const [transcribing, setTranscribing] = useState(false)
  const ref = useRef<HTMLTextAreaElement>(null)
  const dict = dictation.useDictation()
  /** This pane pressed start: the store is shared by every mounted composer, only the owner renders and finishes it. */
  const [owner, setOwner] = useState(false)
  const ownerRef = useRef(owner)
  ownerRef.current = owner
  /** Ours is running: the box is read-only and renders §5.1's spans. */
  const dictating = owner && dict.active !== null
  const dictReason =
    dictation.reason(dict) ?? (dict.active !== null && !owner ? 'Dictating in another pane' : null)
  // 82 §B: the pill's wave — the dictation tap's analyser (remote engine) or frames from the engine's level packets.
  const dictSource = useVoiceSource(dictating && dict.phase === 'dictating', () => tapOrLevels(dictation.tap(), dictation.onLevels))
  /** Caret to place once the textarea is back after a dictation — with the dictated `text` to land there first (138 UNDO). */
  const [pending, setPending] = useState<{ text?: string; caret: number } | null>(null)
  /** 138 UNDO D3: text blocks whose token an undo or a chip ✕ took, waiting for a redo to bring the token back. Per draft. */
  const shelf = useRef<Shelf>(new Map())
  useEffect(() => {
    shelf.current = new Map()
  }, [draftKey])
  const { state: comm, exports } = useCommunication(session)
  // Owned private tree (51 §4.5): uploads go to its scratch root; a private draft has no root yet, so blob uploads
  // (which would land in the durable store) and voice notes (transcribed from the durable store) wait for the session.
  const privateRoot = usePrivate((s) => s.current?.root)
  const priv = session ? privateRoot !== undefined && isPrivate(session) : (draftCreate?.private ?? false)
  const noUpload = priv && !session

  const slashWord = /^\/\S*$/.test(text) ? text : null
  // D15 (b): an ACP session's menu lists the harness's commands beside theuth's, each labelled.
  const harnessMenu = useMemo<HarnessMenu | null>(
    () => (session && agent && harnessCommands?.length ? { agent, commands: harnessCommands, skills: skillIndex?.map((s) => s.name) ?? null } : null),
    [session, agent, harnessCommands, skillIndex],
  )
  // 216: running plugins' `/<plugin>[:<cmd>]` close the menu and count as commands.
  const pluginCommands = usePluginCommands()
  const slashItems = useMemo(() => (slashWord === null ? [] : slashMenu(slashWord, harnessMenu, pluginCommands)), [slashWord, harnessMenu, pluginCommands])
  const slash = slashItems.length > 0
  const held = pastes.size > 0
  /** The attach picker (66 §7.1): `'menu'` from `+`/`theuth:attach-open`, `'at'` from the `@` trigger (§7.8). */
  const [picking, setPicking] = useState<'menu' | 'at' | null>(null)
  const [menu, setMenu] = useState(false)
  /** #3: the `+` menu's suffix steps — `'list'` the saved suffixes, a suffix its scope question; `null` the menu. */
  const [suffixStep, setSuffixStep] = useState<'list' | Suffix | null>(null)
  /** The `+` menu's Skills step (`SkillPicker`). */
  const [skillStep, setSkillStep] = useState(false)
  const box = useRef<HTMLDivElement>(null)
  const qc = useQueryClient()
  const notes = useNotes()
  const newNote = useMutation({
    mutationFn: () => createNote(api, qc, notes.data ?? []),
    onSuccess: (n) => addRef(toRef({ kind: 'note', doc: n })),
    onError: (e) => toast.error(e.message),
  })

  // ⇧Esc lands in the focused pane's composer (or the new-chat draft when it is not in a pane).
  const focused = usePanes((s) => pane === null || s.focused === pane)
  useEffect(() => {
    if (!focused) return
    const focus = () => ref.current?.focus()
    window.addEventListener('theuth:focus-composer', focus)
    return () => window.removeEventListener('theuth:focus-composer', focus)
  }, [focused])

  /** COMPOSER-FOCUS (2026-10-07): the caret comes back after a send on desktop so Esc (`keys.stop`) cancels the turn;
   * never on touch, never away from a dialog or another input (`lib/composer-focus`). */
  const refocus = () => {
    if (shouldRefocus(document.activeElement, box.current, touch())) ref.current?.focus()
  }
  // A new chat's send navigates to `/c/$session`, whose composer mounts fresh: it takes the caret like the one that sent.
  // Keyed on `focused`, not mount: the route's `usePanes.show(session)` runs after this child's effects, so on mount
  // this pane is not the focused one yet. The guard keeps a click into another input or a dialog where it is.
  useEffect(() => {
    if (focused) refocus()
  }, [focused])

  // Dictation (54 §7): Ctrl+D / the Mic button toggles; we own the caret. The local daemon owns the mic; on a
  // relay-linked engine this browser's mic streams to it instead (§4.5). Both voice buttons stay while a turn
  // runs — a note or dictated text queues as a steer through `submit()`, exactly like a typed one.
  const canDictate = voice && !priv && !readOnly
  const dictate = () => {
    if (!canDictate) return
    if (dictating) {
      // Starting/stopping: the press is swallowed; there is no id to stop yet, or a stop is already in flight.
      if (dict.phase === 'dictating') void dictation.stop(api)
      return
    }
    if (dictReason) {
      toast(dictReason)
      return
    }
    if (dict.phase !== 'idle') return
    const ta = ref.current
    const at = ta?.selectionStart ?? text.length
    setOwner(true)
    void dictation.start(api, text.slice(0, at), text.slice(at), useEngines.getState().active !== LOCAL, false, dictationCommands(harnessCommands))
  }
  const dictateRef = useRef(dictate)
  dictateRef.current = dictate
  useEffect(() => {
    if (!focused) return
    const h = () => dictateRef.current()
    window.addEventListener('theuth:dictate', h)
    return () => window.removeEventListener('theuth:dictate', h)
  }, [focused])
  const live = dictating && dict.phase === 'dictating'
  useEffect(() => {
    if (!live) return
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key !== 'Escape') return
      e.preventDefault() // this Esc is spent (the talk view's Esc-to-leave checks it)
      void dictation.discard(api)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [live, api])
  /** 153 §6.1: the dictation whose words are in the box, until the next send reports it (`''`/null = nothing to report). */
  const lastDictation = useRef<string | null>(null)
  /** 153 §7.4 "send it": the landed value to `submit()` once the box says it; `null` otherwise. */
  const [autoSend, setAutoSend] = useState<string | null>(null)
  /** Pending send gesture triggered by pressing Send/Steer while dictating; consumed on dictation finish. */
  const sendAfterDictate = useRef<'press' | 'interrupt' | null>(null)
  /** This pane's identity for `correction` → the suggestion card lands under the pane that sent (§6.3 step 4). */
  const me = useRef({})
  const suggestion = dictation.useSuggestion()
  useEffect(
    () =>
      dictation.onFinish(({ value, caret, id, send }) => {
        if (!ownerRef.current) return
        setPending({ text: value, caret })
        lastDictation.current = id || null
        const pendingSend = sendAfterDictate.current
        sendAfterDictate.current = null
        if (send || pendingSend) setAutoSend(value)
      }),
    [],
  )
  // The spoken "send it" fires once the landed text is the box's text — the same path as ⏎, exactly once.
  useEffect(() => {
    const r = spokenSend(autoSend, text)
    if (!r.fire) return
    setAutoSend(r.auto)
    void submit()
  })
  // Idle again (finished, discarded, or `start` failed) → nobody owns it. `start` flips the phase synchronously,
  // so the same batch that sets `owner` never sees idle.
  useEffect(() => {
    if (dict.phase === 'idle') {
      setOwner(false)
      sendAfterDictate.current = null
    }
  }, [dict.phase])
  /** The chips `next` still carries, through the shelf (138 UNDO D3): a text block whose token left waits there for a redo. */
  const reconcileHeld = (next: string) =>
    setPastes((held) => {
      const r = reconcileShelved(next, held, shelf.current)
      shelf.current = r.shelf
      return r.pastes
    })
  /** Keys taken on the shelf: a new chip never reuses one, or a redo would hand its token the newer block. */
  const floor = () => Math.max(0, ...shelf.current.keys())
  /**
   * Make the box say `next` as one undo step (138 UNDO): the editing command's `input` runs `onChange`, so the store
   * follows. The plain store write instead (not undoable) when the box cannot take it — unmounted, read-only, the
   * command refused — or, under a finger, when it is not focused: focusing to edit would pop the keyboard for a chip ✕.
   * `caret` overrides the command's (after the change) once React has rendered.
   */
  const edit = (next: string, caret?: number) => {
    const ta = ref.current
    if (!ta || (touch() && document.activeElement !== ta) || !applyText(ta, next)) {
      setText(next)
      reconcileHeld(next)
    }
    if (caret !== undefined) setPending({ caret })
  }
  useEffect(() => {
    const ta = ref.current
    if (pending === null || !ta) return
    // The dictation lands once the box is back: one undo step, so ⌘Z takes the dictated words (the steps from
    // before the dictation did not survive the remount).
    if (pending.text !== undefined && !applyText(ta, pending.text)) {
      setText(pending.text)
      reconcileHeld(pending.text)
    }
    ta.setSelectionRange(pending.caret, pending.caret)
    ta.focus()
    setPending(null)
  }, [pending, dictating])

  const addFiles = (paths: string[], dims?: [number, number]) => {
    setPastes((h) => paths.reduce((held, path) => attach(path, dims?.[0] ?? 0, dims?.[1] ?? 0, held, floor()), h))
    const id = model ?? (session ? defaultModel : (useDrafts.getState().pick.model ?? defaultModel))
    const info = models?.find((m) => m.id === id)
    if (info && !info.image && paths.some((p) => IMAGE.test(p)))
      toast.warning(
        visionModel
          ? `${info.name} does not accept images; ${visionModel.id} will inspect them`
          : `${info.name} does not accept images; they will be omitted unless a vision fallback is set (/vision)`,
      )
  }

  /** A queued steer taken back (chip click, the withdraw key): its text in the box, its files held again (171F4). */
  const editQueued = (next: string, files: string[]) => {
    edit(next)
    if (files.length) addFiles(files)
  }

  /** 66 §7.6: attach a reference; its chip stands above the box until send expands it to the marker line. */
  const addRef = (ref: AttachRef) => setPastes((held) => attachRef(ref, held, floor()))
  // `theuth:attach` (66 §7.8, §7.9): "Attach again", "Attach to this chat", the search dialog's row action — and
  // `theuth:attach-open` — land in the focused composer of that session, like `theuth:focus-composer`.
  useEffect(() => {
    if (!focused) return
    const add = (e: Event) => {
      const d = (e as CustomEvent<AttachRequest>).detail
      if (d.session === session) addRef(d.ref)
    }
    const open = (e: Event) => {
      if ((e as CustomEvent<{ session: string | null }>).detail.session === session) setPicking('menu')
    }
    window.addEventListener('theuth:attach', add)
    window.addEventListener('theuth:attach-open', open)
    return () => {
      window.removeEventListener('theuth:attach', add)
      window.removeEventListener('theuth:attach-open', open)
    }
  }, [session, focused])
  /** The `@word` under the caret while the picker is in quiet mode (§7.8). */
  const [atWord, setAtWord] = useState<{ start: number; word: string } | null>(null)
  /** The `@` the user dismissed (Esc, no results): it stays as typed and does not reopen until edited past. */
  const atDismissed = useRef<number | null>(null)
  /** Text changed: open the picker on a fresh `@word`, keep its query in step, close it when the word ends (a space). */
  const changed = (next: string, caret: number) => {
    const t = atTrigger(next, caret)
    if (!t) {
      atDismissed.current = null
      setAtWord(null)
      if (picking === 'at') setPicking(null)
      return
    }
    if (atDismissed.current === t.start) return
    setAtWord(t)
    if (picking === null) setPicking('at')
  }
  const dismissAt = () => {
    if (atWord) atDismissed.current = atWord.start
    setPicking(null)
    setAtWord(null)
  }
  /** The picker's result: chips for every picked reference, the `@word` gone (§7.8), focus back in the box (§7.7). */
  const picked = (refs: AttachRef[]) => {
    setPastes((h) => refs.reduce((held, r) => attachRef(r, held, floor()), h))
    ref.current?.focus()
    if (picking === 'at' && atWord) edit(stripAt(text, atWord.start, atWord.word), atWord.start)
    setPicking(null)
    setAtWord(null)
  }

  // PATH-AC: `path` is the open list; `pathAsk` the debounced request behind it. The engine running the session
  // lists (`path_complete`), so a remote session completes on its own machine; a failure or empty listing shows nothing.
  const [path, setPath] = useState<PathPopup | null>(null)
  const [pathAsk, setPathAsk] = useState<{ text: string; caret: number; now: boolean } | null>(null)
  /** The token start the user closed with Esc: it stays as typed until edited past. */
  const pathDismissed = useRef<number | null>(null)
  /** The listing engine is Windows (its last reply), so `C:\` triggers too. */
  const pathWindows = useRef(false)
  const pathSeq = useRef(0)
  const isMobile = useIsMobile()
  // 183 R1: a narrow desktop pane (a split, a popout) gets the short placeholder by the box's own width, not the window's.
  const narrow = useNarrow(box, NARROW_BOX_PX)
  // The bar's fit: the box's content width is the bar's (`contentRect` leaves out the box's padding).
  const barTight = useNarrow(box, BAR_TIGHT_PX)
  // The model's slot on the bar: the room every button leaves (it is the bar's one `flex-1` item), so the label picks
  // the longest step that fits (`modelStep`) and never pushes a button.
  const modelSlot = useRef<HTMLDivElement>(null)
  const modelRoom = useWidth(modelSlot)
  // The box's top row (`13%  ~$44`): an existing session only, never the new-chat draft or a HUD composer.
  const meta = useComposerMeta(!resolveSession && session ? session : null)
  // The row counts toward the box: the text's max height gives up its 16 px, so a full box is no taller than before.
  const textMax = meta ? (touch() ? 'max-h-[144px]' : 'max-h-[184px]') : touch() ? 'max-h-[160px]' : 'max-h-[200px]'
  const pathChanged = (next: string, caret: number) => {
    const t = pathTrigger(next, caret, pathWindows.current)
    const slashNow = /^\/\S*$/.test(next) && slashMenu(next, harnessMenu, pluginCommands).length > 0
    if (!t || slashNow || picking !== null) {
      pathDismissed.current = null
      setPath(null)
      setPathAsk(null)
      return
    }
    if (pathDismissed.current === t.start) return
    setPathAsk({ text: next, caret, now: false })
  }
  useEffect(() => {
    if (!pathAsk) return
    const seq = ++pathSeq.current
    const run = async () => {
      const t = pathTrigger(pathAsk.text, pathAsk.caret, pathWindows.current)
      if (!t) return
      const cursor = new TextEncoder().encode(pathAsk.text.slice(0, pathAsk.caret)).length
      const r = await api.path_complete(session, pathAsk.text, cursor).catch(() => null)
      if (seq !== pathSeq.current) return
      pathWindows.current = r?.platform === 'windows'
      setPath(r && r.candidates.length > 0 ? { start: t.start, end: t.end, items: r.candidates, truncated: r.truncated, selected: 0 } : null)
    }
    if (pathAsk.now) {
      void run()
      return
    }
    const timer = setTimeout(run, PATH_DEBOUNCE_MS)
    return () => clearTimeout(timer)
  }, [pathAsk, session, api])
  /** Accept row `i`: only the remainder past the typed prefix lands; a directory keeps completing. */
  const pickPath = (i: number) => {
    if (!path) return
    const item = path.items[i]
    if (!item) return
    const next = acceptPath(text, path, item)
    edit(next.text, next.caret)
    setPath(null)
    setPathAsk(item.is_dir ? { text: next.text, caret: next.caret, now: true } : null)
  }
  /** The list is for the draft as it stands: a send or a recall (programmatic text) leaves no stale rows. */
  const pathOpen = path !== null && picking === null && pathTrigger(text, path.end, pathWindows.current)?.start === path.start

  // One file through `put_attachment` (31 §9), or `put_session_attachment` for a private tree, and an image's size read
  // alongside. The landed bytes seed the thumbnail cache, so the held strip shows them without fetching them back.
  const sendFile = useRef<(file: Blob) => Promise<Uploaded>>(null!)
  sendFile.current = async (file) => {
    const owner = priv && session ? session : undefined
    const size = file.type.startsWith('image/')
      ? createImageBitmap(file).then(
          (b): [number, number] => {
            const dims: [number, number] = [b.width, b.height]
            b.close()
            return dims
          },
          () => undefined,
        )
      : undefined
    const [path, dims] = await Promise.all([putAttachment(api, file, extFor(file), owner), size])
    seedFile(path, file, owner)
    return { path, dims }
  }
  const landed = useRef<(u: Uploaded) => void>(null!)
  landed.current = (u) => addFiles([u.path], u.dims)
  // Each file holds a tile in the strip from the moment it is chosen (`uploads.ts`); Send waits while any is uploading.
  const [uploads, uploader] = useUploads(
    (f) => sendFile.current(f),
    (u) => landed.current(u),
  )
  useEffect(() => () => uploader.clear(), [uploader])
  const uploading = uploadWait(uploads)
  const addBlobs = (blobs: (File | Blob)[]) => {
    if (blobs.length === 0) return
    if (noUpload) {
      toast.error('Send the first message of a private session before attaching files')
      return
    }
    uploader.start(blobs)
  }

  // 66 §7.8: a dragged row (`application/x-theuth-attach`) attaches; anything else is files. OS drops arrive here as
  // `File`s and upload like the browser's (BLOCKNOTE-DRAG turned off Tauri's path-reporting drop hook, which also
  // swallowed every in-page HTML5 drop in WKWebView); host paths still come from the Attach dialog.
  const onDrop = (e: DragEvent) => {
    e.preventDefault()
    const ref = e.dataTransfer.getData(DRAG_TYPE)
    if (ref) {
      try {
        addRef(JSON.parse(ref) as AttachRef)
      } catch {
        toast.error('Could not read the dropped item')
      }
      return
    }
    addBlobs([...e.dataTransfer.files])
  }

  const onPaste = async (e: ClipboardEvent<HTMLTextAreaElement>) => {
    const image = [...e.clipboardData.items].find((i) => i.type.startsWith('image/'))?.getAsFile()
    if (image) {
      e.preventDefault()
      addBlobs([image])
      return
    }
    // 165: a rich paste with links becomes markdown; anything else is the plain text, as before.
    const pasted = pasteText({ html: e.clipboardData.getData('text/html'), plain: e.clipboardData.getData('text/plain') })
    if (!pasted) return
    e.preventDefault()
    // The block is held before the edit: the edit's `onChange` reconciles against the store, which must already have it.
    const { selectionStart: a, selectionEnd: b } = e.currentTarget
    const held = tokenize(pasted, pastes, floor())
    if (held.pastes !== pastes) setPastes(held.pastes)
    edit(text.slice(0, a) + held.text + text.slice(b), a + held.text.length)
  }

  const reset = () => {
    setText('')
    setPastes(NONE)
    shelf.current = new Map()
    setSelected(0)
  }

  const ctx: Ctx = {
    api,
    session,
    parent,
    navigate,
    async reload() {
      if (session)
        useSessions
          .getState()
          .loadHistory(session, await api.load_session(session, null, 60), 'replace')
    },
  }

  /** Send `spoken` (a `[voice note …]` line) or the textarea; a voice send enters voice mode, a typed one leaves it (45 §7.2).
   * `typed`: a "Next:" pick — sent as a typed plain message, beside the box: the draft (text, held pastes, parked
   * skills, an unsent retry) stays as it is, and a failed pick is a toast, not the unsent banner.
   * `interject` (⌘/Ctrl+Enter) mid-turn: cancel the turn, then queue; the queued message starts the next turn now.
   * The box keeps its text until the engine acks (70 SEND-NEVER-LOSES): a failure leaves it flagged `unsent` with the
   * banner, and the retry (Retry, or Enter again) reuses the client input id so the engine never takes it twice. */
  const submit = async (spoken?: string, interject = false, typed?: string) => {
    const pick = typed !== undefined
    const line = spoken ?? typed ?? text.trim()
    if ((!line && !skills.length && !held && !carries) || busy || starting || uploading) return
    // 153 §6.1: the typed box after a dictation is what was actually sent — the engine learns single-word edits from it.
    if (spoken === undefined && typed === undefined && lastDictation.current) dictation.correction(api, lastDictation.current, line, me.current)
    lastDictation.current = null
    setBusy(true)
    // Something is sent: the last turn's "Next:" suggestions are spent (typing alone keeps them).
    if (session) useSessions.getState().dismissSuggestions(session)
    setSent((n) => n + 1)
    if (!matchMedia('(prefers-reduced-motion: reduce)').matches) {
      setPlaying(true)
      setTimeout(() => setPlaying(false), 1400)
    }
    const mode = (id: string) => speech.setVoiceMode(id, spoken !== undefined)
    const clientId = (!pick && unsent) || crypto.randomUUID()
    /** The session the input went to (a new chat's once created), and how — for the rollback in `catch`. */
    let target: string | null = session
    let steered = false
    let body = ''
    /** 183 §5.6: a new chat's first message went to another machine (`sendFirstElsewhere` took the user back on failure). */
    let elsewhere = false
    try {
      // D15: a harness command picked from the menu with its arguments typed after it — the harness's, sent as typed.
      if (!pick && session && rawCmd && (line === rawCmd || line.startsWith(`${rawCmd} `))) {
        reset()
        setRawCmd(null)
        await sendRaw(api, session, line)
        return
      }
      if (!pick) setRawCmd(null)
      if (!pick && line.startsWith('/skill:')) {
        const [name = '', ...words] = line.slice('/skill:'.length).trim().split(/\s+/)
        if (!name) throw new Error('usage: /skill:<name>')
        // The words stay in the box: `reset()` then `setText` when this composer stays mounted; a new chat's
        // composer reads them from `drafts` as it mounts.
        await draftSkill(api, navigate, session, name, words.join(' '), suffixChat)
        reset()
        if (session) setText(words.join(' '))
        return
      }
      if (!pick && isCommand(line, skillIndex?.map((s) => s.name) ?? null, pluginCommands)) {
        await run(line, ctx)
        reset()
        return
      }
      // Parked skills: the server builds one input from their `<skill>` bodies and these words — the pinned document
      // leading and the prompt suffix closing them, as a plain send's body — and the `once` suffix is spent with it
      // (`sendParked`). `TurnStarted` (idle) or `InputQueued` (running) echoes it as the skill card, so nothing is
      // marked sent here. The attached files ride on the same input, as a plain send's do; the box (words and files)
      // clears once the engine took it, and a failure leaves it for a retry.
      if (!pick && session && skills.length) {
        const { text: words, files } = expand(line, pastes)
        mode(session)
        if (running && interject) await api.cancel(session, null, 'input')
        await sendParked(api, session, pinned ? compose([pinned], words) : words, suffixChat, files)
        reset()
        return
      }
      const expanded = expand(line, pick ? EMPTY.pastes : pastes)
      // 59 §7.3: the pinned document leads every message, so its id survives compaction.
      // #3: the chat's prompt suffix closes every typed or spoken message and a skill send (never a command).
      body = withSuffix(pinned ? compose([pinned], expanded.text) : expanded.text, suffix?.text)
      const { files } = expanded
      const { markSent, applyEvents, loadHistory } = useSessions.getState()
      // 66 §7.6: recents are recorded on send, not on pick.
      const sent = [...pastes.values()].flatMap((p) => (p.kind === 'ref' ? [p.ref] : []))
      if (sent.length > 0) saveRecents(active, pushRecents(loadRecents(active), sent))
      // A voice note has no text in the box: park the line there so a failure has something to restore.
      if (!pick) {
        if (spoken && !text) setText(spoken)
        useDrafts.getState().sending(draftKey, clientId)
      }
      if (deliver) {
        if (session && running && interject) await api.cancel(session, null, 'input')
        target = await deliver({ text: body, files, clientId })
      } else if (session && running) {
        steered = true
        // 171F4 N7: the chip and then the `you` row show the attached files at once (the engine's echo has none).
        applyEvents([{ type: 'InputQueued', session, id: clientId, text: body, defer: false, files }])
        mode(session)
        // Cancel first: its epoch bump would stale a steer admitted before it (todo auto-continue).
        if (interject) await api.cancel(session, null, 'input')
        await deadline(api.steer(session, body, clientId, files))
      } else if (session) {
        markSent(session, body, files)
        mode(session)
        await deadline(api.send_input(session, body, files, null, clientId))
      } else if (resolveSession) {
        // 128 R6: the HUD's composer — the first send goes to an existing session named on demand (theuth's desk), not a new one.
        const id = await resolveSession()
        target = id
        markSent(id, body, files)
        mode(id)
        await deadline(api.send_input(id, body, files, null, clientId))
      } else {
        if (draftCreate?.private && (!client || !supports(client.discovery, privateRequirements))) throw new UnsupportedCapability('private sessions')
        if (draftCreate?.endless && (!client || !supports(client.discovery, endlessRequirements))) throw new UnsupportedCapability('fractal context')
        if (draftCreate?.private && !usePrivate.getState().acknowledged)
          throw new Error('Acknowledge the private session notice first')
        // A switch on needs `create_session` with the pick in it; `openDraftSession` takes a bare pick there too, else `new_session()`.
        // 183 §5.6: on another machine the chat opens at once with the message pending, and a failure comes back to this
        // draft with the text (a split pane's draft leaf via `onUncreated`).
        await sendFirst({
          api,
          options: draftCreate && (draftCreate.private || draftCreate.project || draftCreate.endless) ? createOptions(draftCreate, useDrafts.getState().pick) : null,
          input: clientId,
          machine: (engine) => (engine !== mesh?.self_id ? engineLabel(engine, mesh?.peers) : null),
          created: async (id, open, pick) => {
            if (open?.retention === 'private') usePrivate.getState().open(open)
            target = id
            if (sessionTitle) await api.rename_session(id, sessionTitle)
            // Seed before sending: a fast failure can finish before the RPC reply.
            loadHistory(id, { ...FRESH, model: pick.model ?? defaultModel ?? '', effort: pick.effort ?? defaultEffort ?? null }, 'replace')
            markSent(id, body, files)
            mode(id)
          },
          send: (id) => deadline(api.send_input(id, body, files, null, clientId)),
          ...(!onCreated
            ? {
                open: async (id: string) => {
                  elsewhere = true
                  await navigate({ to: '/c/$session', params: { session: id } })
                },
                back: () => navigate({ to: '/new' }),
              }
            : onUncreated
              ? {
                  open: async (id: string) => {
                    elsewhere = true
                    onCreated(id)
                  },
                  back: async (id: string) => onUncreated(id),
                }
              : {}),
        })
      }
      if (!pick) useDrafts.getState().acked(draftKey, spoken ?? text)
      // #3: a "this message" suffix is spent; an "every message" one follows a new chat onto its session.
      useSuffixes.getState().sent(suffixChat)
      if (target && !session && !resolveSession) useSuffixes.getState().carry(suffixChat, target)
      retried.current = false
      ownerRetried.current = false
      if (!pick) shelf.current = new Map()
      setSelected(0)
      // A retry the engine already had starts no turn, so the row shown at once would stay a ghost: drop it. A
      // routed retry is either stamped by its `TurnStarted` already (then this is a no-op) or echoed by it after.
      // A steer's chip self-heals: the engine's `InputInjected` for the same id removes it.
      if (!pick && unsent && target && !steered) useSessions.getState().unmarkSent(target, body)
      if (deliver) onSent?.(target ?? '')
      else if (target && !session && !elsewhere) {
        if (onCreated) onCreated(target)
        else await navigate({ to: '/c/$session', params: { session: target } })
      } else if (session) onSent?.(session)
    } catch (e) {
      // The row shown at once comes back off; the text is still in the box, flagged `unsent` for the banner.
      const st = useSessions.getState()
      if (target && steered) st.applyEvents([{ type: 'InputWithdrawn', session: target, id: clientId }])
      else if (target) st.unmarkSent(target, body)
      if (target && !session && !elsewhere) {
        // The chat exists now: the unsent draft follows it, so Retry lands in that session.
        const drafts = useDrafts.getState()
        drafts.set(`${onCreated ? 'panel:' : ''}${target}`, drafts.by[draftKey] ?? EMPTY)
        drafts.set(draftKey, EMPTY)
        useSuffixes.getState().carry(suffixChat, target)
        if (onCreated) onCreated(target)
        else await navigate({ to: '/c/$session', params: { session: target } })
      }
      if ((elsewhere && e instanceof Error ? e.cause : e) instanceof SendTimeout) useConnection.getState().client?.probe('send-timeout')
      const unreachable = ownerUnreachable(e)
      const draining = sessionDraining(e)
      if (elsewhere) toast.error(e instanceof Error ? e.message : String(e))
      else if (draining && target && !retried.current) {
        // 139 §9.3: the session is mid-release on this engine — one automatic retry after 2 s, no card.
        retried.current = true
        toast.message(
          draining.nextOwner ? `moving to ${engineLabel(draining.nextOwner, mesh?.peers)} — retrying…` : `pausing for ${draining.reason} — retrying…`,
          { duration: 2000 },
        )
        setTimeout(() => void submit(spoken, interject, typed), 2000)
      } else if (unreachable && target) useOwnerOffline.getState().park(target, unreachable)
      else if (target && retryNotOwner(e, target, ownerRetried, () => void submit(spoken, interject, typed), () => toast.error(e instanceof Error ? e.message : String(e)))) {
        // 183 §13 row 2: the owner changed under the send — the draft stays in the box; it goes once more when the new owner is known.
      } else toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
      refocus()
    }
  }

  /** Item 8: ⌘↩ / a long press with an empty box while steers are queued — cancel the turn and deliver them now,
   * in order, as the next input. Withdraw before cancel: cancel's epoch bump would stale them where they stand. */
  const flush = async () => {
    if (!session || !queued?.length || busy) return
    setBusy(true)
    try {
      // `withdraw` answers the text only: the files come from our queue (empty after a reload).
      const inputs: { body: string; files: string[] }[] = []
      for (const q of queued) inputs.push({ body: await api.withdraw(session, q.id), files: q.files ?? [] })
      await api.cancel(session, null, 'input')
      const { applyEvents } = useSessions.getState()
      for (const { body, files } of inputs) {
        const id = crypto.randomUUID()
        applyEvents([{ type: 'InputQueued', session, id, text: body, defer: false, files }])
        await api.steer(session, body, id, files)
      }
    } catch (e) {
      if (!parkIfOffline(session, e, flush)) toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
      refocus()
    }
  }

  // The composer's phase and its two gestures (`lib/composer-state`): Enter / the button, and ⌘↩ / a long press.
  const dictatedFilled = Boolean(
    dictating &&
      dict.active &&
      (dict.active.before.trim() || dict.active.after.trim() || dict.active.utts.some((u) => dictation.shown(u).trim() !== '')),
  )
  const filled = Boolean(text.trim() || skills.length || held || dictatedFilled || carries)
  const at = phase(Boolean(session && running), queued?.length ?? 0)
  const gesture = (g: 'press' | 'interrupt') => {
    if (dictating) {
      if (!filled) return
      sendAfterDictate.current = g
      if (dict.phase === 'dictating') void dictation.stop(api)
      return
    }
    const a = g === 'press' ? press(at, filled) : interrupt(at, filled)
    if (a === 'flush') void flush()
    else if (a) void submit(undefined, a === 'interject')
  }
  const steerPress = useLongPress(
    () => gesture('press'),
    () => gesture('interrupt'),
  )

  // 133 §4: a stop is scoped. Tap / Esc → the root turn; the button held and slid up, Esc held 700 ms, or
  // Esc ×3 → the root and every running descendant. N comes from the session tree the store already holds.
  const children = useSessionField(session ?? '', 'children')
  const descendants = useMemo(() => {
    const { shown, more } = stripRows(children ?? {})
    return shown.length + more
  }, [children])
  const stop = (scope: CancelScope) => {
    if (!session) return
    // 183 R1: a cancel parked behind the OwnerOfflineCard (`routed` resolves undefined) stopped nothing: no "Stopped" toast.
    routed(session, async () => (await api.cancel(session, scope, 'stop'), true)).then(
      (ran) => ran && scope === 'tree' && toast(`Stopped ${descendants} agent${descendants === 1 ? '' : 's'}`),
      (e: unknown) => toast.error(e instanceof Error ? e.message : String(e)),
    )
  }
  const escState = useRef<StopState>(STOP_IDLE)
  /** The key of the Esc chord while it is down (`null` between presses). */
  const [escKey, setEscKey] = useState<string | null>(null)
  const [ring, setRing] = useState(0)
  const escAct = (a: StopAct) => {
    const [next, verb] = stepStop(escState.current, a)
    escState.current = next
    if (verb) stop(verb)
    setRing(stopProgress(next, a.t))
  }
  // While Esc is down: a tick for the ring (and the 700 ms stop when the OS sends no key repeats); keyup or blur ends it.
  useEffect(() => {
    if (escKey === null) return
    const tick = window.setInterval(() => escAct({ type: 'tick', t: performance.now() }), 50)
    const end = () => {
      setEscKey(null)
      escAct({ type: 'up', t: performance.now() })
    }
    const onUp = (e: globalThis.KeyboardEvent) => e.key === escKey && end()
    window.addEventListener('keyup', onUp)
    window.addEventListener('blur', end)
    return () => {
      clearInterval(tick)
      window.removeEventListener('keyup', onUp)
      window.removeEventListener('blur', end)
    }
  }, [escKey])
  useEffect(() => {
    if (running) return
    escState.current = STOP_IDLE
    setEscKey(null)
    setRing(0)
  }, [running])

  // Mic (45 §7.2): click to record, click again to upload → `transcribe` → send as `[voice note, N s] …`; Escape discards.
  const sendNote = async (blob: Blob) => {
    setTranscribing(true)
    try {
      const t = await api.transcribe(await putAttachment(api, blob, extFor(blob)))
      if (!t.text.trim()) {
        toast('Nothing recognized')
        return
      }
      await submit(noteText(t.text, Number(t.duration_ms)))
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setTranscribing(false)
    }
  }

  useEffect(() => {
    if (!recording) return
    setSecs(0)
    const tick = setInterval(() => setSecs((s) => s + 1), 1000)
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key !== 'Escape') return
      e.preventDefault() // this Esc is spent (the talk view's Esc-to-leave checks it)
      stopRecording(true)
    }
    window.addEventListener('keydown', onKey)
    return () => {
      clearInterval(tick)
      window.removeEventListener('keydown', onKey)
      // Unmounted mid-recording: the recorder's `onstop` may never reach this pane; free the analyser and its context.
      dropNoteWave()
    }
  }, [recording])

  const fail = (e: unknown) => toast.error(e instanceof Error ? e.message : String(e))
  /** A menu row: a harness command (D15) is sent to the harness as typed — at once, or after its arguments when it takes some. */
  const pick = (c: SlashItem) => {
    setSelected(0)
    if (c.raw && session) {
      if (c.hint) {
        edit(`${c.name} `)
        setRawCmd(c.name)
        return
      }
      reset()
      void sendRaw(api, session, c.name).catch(fail)
      return
    }
    edit(`${c.name} `)
    if (c.hint === '') {
      reset()
      void run(c.name, ctx).catch(fail)
    }
  }

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (slash && slashWord) {
      const list = slashItems
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault()
        setSelected((i) => (i + (e.key === 'ArrowDown' ? 1 : list.length - 1)) % list.length)
        return
      }
      if (e.key === 'Tab' || (e.key === 'Enter' && !e.shiftKey)) {
        e.preventDefault()
        const c = list[selected] ?? list[0]
        if (c) pick(c)
        return
      }
    }
    if (path && pathOpen) {
      const action = pathKey(e, e.currentTarget.selectionStart === text.length)
      if (action) {
        e.preventDefault()
        if (action === 'up' || action === 'down') setPath(stepPath(path, action === 'up' ? -1 : 1))
        else if (action === 'accept') pickPath(path.selected)
        else {
          pathDismissed.current = path.start
          setPath(null)
          setPathAsk(null)
        }
        return
      }
    }
    const k = e.nativeEvent
    if (session && running && chord(keys.stop, k)) {
      e.preventDefault()
      if (!k.repeat) setEscKey(k.key)
      escAct({ type: 'down', t: performance.now(), repeat: k.repeat })
      return
    }
    if (session && queued?.length && chord(keys.withdraw, k)) {
      e.preventDefault()
      const last = queued[queued.length - 1]
      if (last) void withdrawInto(api, session, last, editQueued)
      return
    }
    if (e.nativeEvent.isComposing) return
    // A rebindable New line (`insertLineBreak` keeps undo); the defaults leave the textarea's own Enter/Shift+Enter alone.
    if (chord(keys.newline, k)) {
      if (k.key !== 'Enter' || k.altKey || k.ctrlKey || k.metaKey) {
        e.preventDefault()
        document.execCommand('insertLineBreak')
      }
      return
    }
    // Fingers: Enter breaks a line and only the send button sends (31 §8.1).
    if (touch()) return
    if (chord(keys.interject, k)) {
      e.preventDefault()
      gesture('interrupt')
    } else if (chord(keys.send, k)) {
      e.preventDefault()
      gesture('press')
    }
  }

  // A "Next:" pick is sent at once through `submit`, exactly as if typed and sent; `take` makes it one send.
  useEffect(() => {
    if (!session || nextPick === undefined || busy || starting) return
    const item = useNextPick.getState().take(session)
    if (item !== undefined) void submit(undefined, false, item)
  }, [session, nextPick, busy, starting])

  return (
    <div className={cn('mx-auto w-full max-w-[768px]', compact ? 'px-2 pb-1' : 'px-4 pb-2')}>
      {!compact && <CommunicationNotice session={session} state={comm} exports={exports} />}
      {session && offline && (
        <OwnerOfflineCard
          session={session}
          offline={offline}
          onDone={() => useOwnerOffline.getState().clear(session)}
          onResend={async () => void (offline.retry ? await offline.retry() : await submit())}
        />
      )}
      {(starting || creatingOn) && (
        <div role="status" className="mb-2 flex items-center gap-2 px-1 text-sm text-fg-secondary">
          <Loader2 className="size-4 shrink-0 animate-spin" aria-hidden="true" />
          Starting on {starting ?? creatingOn}…
        </div>
      )}
      {session && unsent && !busy && !offline && (
        <div
          role="alert"
          className="mb-2 flex items-center gap-2 rounded-xl border border-warning-border/30 bg-warning-bg/40 px-3 py-2 text-sm"
        >
          <CircleAlert className="size-4 shrink-0 text-warning" aria-hidden="true" />
          <span className="min-w-0 flex-1 text-fg-secondary">Not sent — your message is still here.</span>
          <Button size="sm" variant="outline" onClick={() => useDrafts.getState().set(draftKey, EMPTY)}>
            Discard
          </Button>
          <Button size="sm" onClick={() => void submit()}>
            Retry
          </Button>
        </div>
      )}
      {session && <QueuedChips session={session} onEdit={editQueued} />}
      <PluginCommandCard session={session} />
      {/* 215 C2/G5: skills and the prompt suffix share one left-aligned, wrapping row. */}
      <PillRow
        skills={skills}
        onRemoveSkill={(name) => session && useSkillDraft.getState().remove(session, name)}
        suffix={suffix}
        onClearSuffix={() => useSuffixes.getState().clear(suffixChat)}
      />
      {session && !compact && <ContextPicker session={session} />}
      <input
        ref={fileInput}
        type="file"
        accept="image/*,application/pdf"
        multiple
        className="hidden"
        onChange={(e) => {
          addBlobs([...(e.target.files ?? [])])
          e.target.value = ''
        }}
      />
      <AttachPicker
        session={session}
        open={picking !== null}
        at={picking === 'at' ? (atWord?.word ?? '') : null}
        anchor={box}
        onClose={() => {
          if (picking === 'at') dismissAt()
          else setPicking(null)
          ref.current?.focus()
        }}
        onAttach={picked}
        onEmpty={dismissAt}
      />
      {strip}
      <div
        ref={box}
        data-composer=""
        data-tour="chat-composer"
        className={cn('relative rounded-2xl border border-border-subtle bg-elevated px-3 pb-2 shadow-sm', meta ? 'pt-1' : 'pt-2.5')}
        onDragOver={(e) => e.preventDefault()}
        onDrop={onDrop}
      >
        {/* Row 0: `13%  ~$44`, a thin row inside the top of the box; the text starts under it at the full width. A voice
            note keeps the row's height, empty (no jump); a chat with no reading and no cost (or a draft) has no row. */}
        {meta && session && <ComposerMeta session={session} meta={meta} hidden={stage !== 'idle'} />}
        {slash && <SlashList items={slashItems} selected={selected} onPick={pick} />}
        {path && pathOpen && <PathList popup={path} mobile={isMobile} onPick={pickPath} />}
        <HeldStrip
          pastes={pastes}
          pinned={pinned}
          onChange={setPastes}
          onRemove={(i) => {
            edit(untoken(text, i)) // a text block: its token goes and `reconcileHeld` shelves the block for a redo
            setPastes((held) => drop(i, held)) // a file or reference chip has no token
          }}
          uploads={uploads}
          onCancelUpload={uploader.cancel}
          onRetryUpload={uploader.retry}
        />
        {/* 215 C2: row 1, the text region at the box's full width. */}
        <div className="relative min-w-0">
          {/* 82 §C: while a voice note records the wave is the box; the textarea stays mounted underneath. */}
          <RecordingBox recording={recording} source={noteSource}>
            {dictating && dict.active ? (
              // 94 DICTATION-LAYOUT: text on top at full width, the voice bar (wave + stop) spread underneath.
              <div className="flex flex-col">
                <DictatedText
                  before={dict.active.before}
                  after={dict.active.after}
                  utts={dict.active.utts}
                  className={cn(BOX, textMax)}
                />
                <Tooltip>
                  <TooltipTrigger asChild>
                    <button
                      type="button"
                      className="mt-1 flex h-9 w-full items-center overflow-hidden rounded-lg bg-transparent px-1 text-fg-secondary"
                      aria-label="Stop dictation"
                      onClick={dictate}
                    >
                      <VoiceWave state="listening" source={dictSource} label="Dictation level" className="h-full flex-1" />
                      <span className="flex size-8 shrink-0 items-center justify-center rounded-full hover:bg-surface-hover">
                        <Square className="size-4 fill-current" />
                      </span>
                    </button>
                  </TooltipTrigger>
                  <TooltipContent>Stop dictation (Ctrl+D)</TooltipContent>
                </Tooltip>
                {dict.active.device && (
                  <p role="status" className="mt-1 px-2 text-xs text-fg-tertiary">
                    {dict.active.device.state === 'lost' ? 'mic lost — reconnecting…' : `mic changed → ${dict.active.device.name ?? 'default'}`}
                  </p>
                )}
              </div>
            ) : (
              <Textarea
                readOnly={readOnly}
                dictate={false}
                ref={ref}
                value={text}
                onChange={(e) => {
                  setText(e.target.value)
                  reconcileHeld(e.target.value)
                  setSelected(0)
                  changed(e.target.value, e.target.selectionStart)
                  pathChanged(e.target.value, e.target.selectionStart)
                }}
                onKeyDown={onKeyDown}
                onPaste={onPaste}
                rows={1}
                placeholder={placeholderFor({ readOnly, running, exports: exports.length > 0, skill: skills.join(', ') || undefined, runsOn, short: isMobile || touch() || narrow })}
                // ponytail: `field-sizing: content` grows the box to `textMax` (less the meta row's 16 px when it shows), then scrolls; older WebKit keeps one line.
                className={cn(BOX, textMax)}
              />
            )}
          </RecordingBox>
          {(stage === 'held' || stage === 'cancelled') && <CancelHint ref={hint} />}
        </div>
        {/* 215 C2: row 2, the toolbar under the full-width text — Add, MCP and the shield left, then the model and
            talk, mic and send right. It never overflows down to the 280 px pane floor: the model label shortens in steps
            to the room the buttons leave (`Opus 5.5`, `Opus`, a sparkle icon); on a narrow bar (`barFit`) the gaps
            tighten and the round buttons go from 36 to 32 px. Every button keeps its own conditions; none is dropped
            for room. */}
        <div data-composer-bar="" className={cn('mt-1 flex items-center gap-1', barFit(barTight))}>
          {/* 89 VOICE-HOLD: a held or locked note takes the row — Delete (locked) · dot + timer stand in for Add. */}
          {stage === 'idle' && lead}
          {stage === 'locked' && <DeleteNote onClick={() => stopRecording(true)} />}
          {stage !== 'idle' && <RecordingMeta secs={secs} stage={stage} />}
          <Popover
            open={menu}
            onOpenChange={(open) => {
              setMenu(open)
              if (!open) {
                setSuffixStep(null)
                setSkillStep(false)
              }
            }}
          >
            <PopoverTrigger asChild>
              <Button
                variant="ghost"
                className={cn('size-9 shrink-0 rounded-full text-fg-secondary', stage !== 'idle' && 'hidden')}
                aria-label="Add"
              >
                <Plus className="size-5" />
              </Button>
            </PopoverTrigger>
            <PopoverContent align="start" className={cn('p-1', skillStep ? 'w-72' : 'w-52')}>
              {skillStep ? (
                <SkillPicker skills={skillIndex} parked={skills} onPick={pickSkill} onBack={() => setSkillStep(false)} />
              ) : suffixStep === 'list' ? (
                <SuffixList
                  suffixes={suffixes}
                  onPick={setSuffixStep}
                  onBack={() => setSuffixStep(null)}
                  onManage={() => {
                    setMenu(false)
                    setSuffixStep(null)
                    void navigate({ to: '/settings/$tab', params: { tab: 'suffixes' } })
                  }}
                />
              ) : suffixStep ? (
                <SuffixScopes
                  suffix={suffixStep}
                  onPick={(scope) => {
                    useSuffixes.getState().activate(suffixChat, suffixStep.id, scope)
                    setMenu(false)
                    setSuffixStep(null)
                    ref.current?.focus()
                  }}
                />
              ) : (
                <>
                  <Button
                    variant="ghost"
                    className="w-full justify-start"
                    onClick={() => {
                      setMenu(false)
                      setPicking('menu')
                    }}
                  >
                    <Paperclip /> Attach…
                  </Button>
                  <Button
                    variant="ghost"
                    className="w-full justify-start"
                    onClick={async () => {
                      if (!hostFiles) {
                        fileInput.current?.click()
                        return
                      }
                      const picked = await pickFiles({
                        multiple: true,
                        filters: [
                          {
                            name: 'Images and PDF',
                            extensions: ['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp', 'pdf'],
                          },
                        ],
                      })
                      if (picked) addFiles(picked)
                    }}
                  >
                    <FileUp /> Attach file
                  </Button>
                  <Button
                    variant="ghost"
                    className="w-full justify-start"
                    disabled={newNote.isPending}
                    onClick={() => {
                      setMenu(false)
                      newNote.mutate()
                    }}
                  >
                    <StickyNote /> New note
                  </Button>
                  <SkillsEntry onOpen={() => setSkillStep(true)} />
                  <Button variant="ghost" className="w-full justify-start" onClick={() => setSuffixStep('list')}>
                    <MessageSquarePlus /> Add prompt suffix
                    <ChevronRight className="ml-auto" />
                  </Button>
                </>
              )}
            </PopoverContent>
          </Popover>
          {/* #4: this chat's MCP servers, beside `+`; a HUD composer sends into a chat named on demand, so it has none. */}
          {!readOnly && !resolveSession && <McpMenu session={session ?? ''} className={cn(stage !== 'idle' && 'hidden')} />}
          {/* How full this chat's context is, and compact / shake / clear: between tools and the shield. */}
          {!readOnly && !resolveSession && session && stage === 'idle' && <ContextButton session={session} />}
          {/* This chat's approval level, right of the context button: the one composer item that never sheds on a narrow bar. */}
          {!readOnly && !resolveSession && session && stage === 'idle' && <ApprovalShield session={session} />}
          {/* 215 C2: the spacer, and the model's slot at its right end: `flex-1` from a 0 basis, so its width is exactly
              the room left, whatever the label shows. `-ml-1` cancels its own gap — 4px a 280px pane needs. A HUD
              composer's chat is named on demand, so no picker; a voice note takes the row, so it goes meanwhile. */}
          <div ref={modelSlot} data-model-slot="" className="-ml-1 flex min-w-0 flex-1 justify-end">
            {!resolveSession && stage === 'idle' && <ComposerModel session={session} own={ownModel} room={modelRoom} />}
          </div>
          {/* 76 §0.12: the former voice-note button is Talk — the live spoken conversation on this session. B3: while it
              runs here, the chat view keeps a compact indicator: tap opens the talk view, the x ends the talk. */}
          {talkHere ? (
            <div className="flex shrink-0 items-center rounded-lg bg-fg-accent/10">
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    className="size-9 shrink-0 rounded-full text-fg-accent"
                    aria-label="Talk running — open"
                    onClick={() => void navigate({ to: '/talk/$session', params: { session } })}
                  >
                    <AudioLines className="size-5 motion-safe:animate-pulse" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>Talking here — open</TooltipContent>
              </Tooltip>
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button variant="ghost" className="size-9 shrink-0 rounded-full text-fg-secondary" aria-label="End talk" onClick={() => void talk.stop(api)}>
                    <X className="size-4" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>End talk</TooltipContent>
              </Tooltip>
            </div>
          ) : canDictate && !dictating && stage === 'idle' && !text.trim() && (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant="ghost"
                  className={cn('size-9 shrink-0 rounded-full', talkElsewhere && 'opacity-50')}
                  aria-label="Talk"
                  aria-disabled={talkElsewhere}
                  disabled={busy || transcribing}
                  onClick={() =>
                    void talk.open(api, session).then(
                      (target) => navigate({ to: '/talk/$session', params: { session: target } }),
                      (e: unknown) => toast.error(e instanceof Error ? e.message : String(e)),
                    )
                  }
                >
                  <AudioLines className="size-5" />
                </Button>
              </TooltipTrigger>
              <TooltipContent>{talkElsewhere ? `Talking in ${talkTitle || 'another chat'} — open` : 'Talk'}</TooltipContent>
            </Tooltip>
          )}
          {/* 94 DICTATION-LAYOUT: while dictating the stop control lives in the voice bar under the text; nothing here. */}
          {canDictate && !(dictating && dict.active) && (
            // 89 VOICE-HOLD: tap → dictation (`dictReason` says why not, via the tooltip); hold → voice note, slide up locks, left cancels.
            <MicButton
              recording={recording}
              source={noteSource}
              hint={hint}
              onStage={setStage}
              disabled={busy || transcribing}
              reason={dictReason}
              onTap={dictate}
              onRecord={() => void record()}
              onStop={stopRecording}
            />
          )}
          {/* 89 VOICE-HOLD: while a note is held or locked the row is the recording's; these come back on reset. */}
          {stage === 'idle' &&
            (at !== 'idle' && session && !playing ? (
              <>
                {/* Item 7: a finger's steer. Tap queues the box into the running turn; hold interrupts (with the box,
                    or the queued steers when it is empty). Not `disabled`: an empty box still takes the hold. */}
                {(filled || at === 'queued') && (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        className={cn(
                          'size-9 shrink-0 rounded-full bg-transparent p-0 text-fg-primary hover:bg-transparent hover:opacity-80 select-none touch-none',
                          (!filled || busy || readOnly || uploading) && 'opacity-50',
                        )}
                        aria-label="Steer"
                        aria-disabled={!filled || busy || readOnly || !!uploading}
                        {...steerPress}
                      >
                        <SendMark pulse={sent} className="size-8" />
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>
                      {uploading ?? (touch() ? 'Steer · hold to interrupt' : `Steer (${chordLabel(keys.send)}) · hold or ${chordLabel(keys.interject)} to interrupt`)}
                    </TooltipContent>
                  </Tooltip>
                )}
                <StopButton count={descendants} ring={ring} onStop={stop} />
              </>
            ) : (
              <SendButton disabled={!filled || busy || readOnly || !!starting} waiting={uploading} onSend={() => gesture('press')} sent={sent} playing={playing} />
            ))}
        </div>
      </div>
      {suggestion && suggestion.reporter === me.current && <DictionarySuggestion s={suggestion} api={api} className="mt-2" />}
    </div>
  )
}

/**
 * The read-only box while dictating (54 §5.1): `before`, a separating space when `before` does not end in one,
 * each utterance's shaped text (153 §3.1; muted until the VAD closes it), a space when `after` does not start with
 * one or punctuation, `after`. The spacing rules mirror `splice()`; the caret is hidden and nothing here takes input.
 */
function DictatedText({
  before,
  after,
  utts,
  className,
}: {
  before: string
  after: string
  utts: dictation.Utt[]
  className: string
}) {
  const spoken = utts.filter((u) => dictation.shown(u) !== '')
  const lead = spoken.length && before && !/\s$/.test(before) ? ' ' : ''
  const trail = spoken.length && after && !/^[\s.,;:!?)]/.test(after) ? ' ' : ''
  return (
    <div
      role="textbox"
      aria-readonly
      aria-live="polite"
      className={cn(className, 'overflow-y-auto whitespace-pre-wrap break-words')}
    >
      {before}
      {lead}
      {spoken.map((u, i) => (
        <Fragment key={i}>
          {i > 0 && ' '}
          <span className={cn('transition-colors duration-150', !u.final && 'text-fg-tertiary')}>{dictation.shown(u)}</span>
        </Fragment>
      ))}
      {trail}
      {after}
    </div>
  )
}

/**
 * The idle Send. `waiting` (an attachment still uploading, `uploadWait`) disables it and says why in a tooltip: a
 * disabled button gets no pointer events, so the tooltip hangs on a focusable wrapper.
 */
export function SendButton({
  disabled,
  waiting,
  onSend,
  sent = 0,
  playing = false,
}: { disabled: boolean; waiting: string | null; onSend(): void; sent?: number; playing?: boolean }) {
  const button = (
    <Button
      className={cn('size-9 shrink-0 rounded-full bg-transparent p-0 text-fg-primary hover:bg-transparent hover:opacity-80', playing && 'disabled:opacity-100')}
      aria-label="Send"
      disabled={disabled || waiting !== null}
      onClick={onSend}
    >
      <SendMark pulse={sent} className="size-8" />
    </Button>
  )
  if (waiting === null) return button
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <span data-send-waiting="" tabIndex={0} aria-label={waiting} className="shrink-0 rounded-full outline-none focus-visible:ring-2 focus-visible:ring-ring/50">
          {button}
        </span>
      </TooltipTrigger>
      <TooltipContent>{waiting}</TooltipContent>
    </Tooltip>
  )
}
