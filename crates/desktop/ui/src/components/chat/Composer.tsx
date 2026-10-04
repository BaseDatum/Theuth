import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { open as pickFiles } from '@tauri-apps/plugin-dialog'
import {
  ArrowUp,
  AudioLines,
  CircleAlert,
  FileUp,
  Megaphone,
  Paperclip,
  Plus,
  Sparkles,
  Square,
  StickyNote,
  X,
} from 'lucide-react'
import {
  type ClipboardEvent,
  type DragEvent,
  Fragment,
  type KeyboardEvent,
  type SyntheticEvent,
  useEffect,
  useRef,
  useState,
} from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Textarea } from '@/components/ui/textarea'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import * as dictation from '@/lib/dictation'
import { extFor, putAttachment } from '@/lib/files'
import { usePanes } from '@/lib/panes'
import { type AttachRef, type AttachRequest, DRAG_TYPE, atTrigger, compose, toRef } from '@/lib/attach'
import { loadRecents, pushRecents, saveRecents, stripAt } from '@/lib/attach-picker'
import { attach, attachRef, drop, expand, type Pastes, reconcile, tokenize, untoken } from '@/lib/pastes'
import { isDesktop, touch } from '@/lib/platform'
import { type OwnerUnreachable, ownerUnreachable } from '@/lib/owner'
import { VIEW, isPrivate, usePrivate } from '@/lib/private'
import { queryKeys, useApi } from '@/lib/rpc'
import { EMPTY, useDrafts, useSkillDraft } from '@/lib/drafts'
import { DEBOUNCE_MS as PATH_DEBOUNCE_MS, type PathPopup, accept as acceptPath, pathKey, pathTrigger, step as stepPath } from '@/lib/path-complete'
import { useIsMobile } from '@/hooks/use-mobile'
import { interrupt, phase, press } from '@/lib/composer-state'
import * as speech from '@/lib/speech'
import * as talk from '@/lib/talk'
import { label as chordLabel, matches as chord } from '@/lib/chord'
import { useShortcuts } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'
import { noteText } from '@/lib/voice-note'
import { MIN_NOTE_MS } from '@/lib/hold'
import { LOCAL, useEngines } from '@/link/engines'
import { useConnection } from '@/store/connection'
import { useSessionField, useSessions } from '@/store/sessions'
import { createNote } from '@/components/notes/create'
import { useNotes } from '@/components/notes/useNotes'
import { AttachPicker } from './AttachPicker'
import { CommunicationNotice, useCommunication } from './CommunicationNotice'
import { openBroadcastDialog } from './BroadcastDialog'
import { ContextPicker } from './ContextPicker'
import { MicButton } from './MicButton'
import { CancelHint, DeleteNote, RecordingMeta, type Stage } from './VoiceHold'
import { HeldStrip } from './HeldStrip'
import { OwnerOfflineCard } from './OwnerOfflineCard'
import { RecordingBox, VoiceWave, useVoiceSource } from '@/components/voice/VoiceWave'
import type { FrameSource, VisualFrame } from '@/lib/audio/types'
import { analyserSource, tapOrLevels } from '@/lib/visual-source'
import { QueuedChips } from './QueuedChips'
import { type Ctx, FRESH, draftSkill, isCommand, matches, run } from './slash'

/** New Chat options (51 §8.3): `private` selects memory-only retention; `project` is root metadata. */
export type DraftCreate = { private: boolean; project: string | null }

const NONE = EMPTY.pastes
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
/** Recorder containers in preference order (45 §7.2): WebKit gives AAC in MP4, Chromium Opus in WebM; the engine decodes all. */
const AUDIO_MIMES = ['audio/mp4', 'audio/webm;codecs=opus', 'audio/ogg;codecs=opus', 'audio/webm']
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
/** The edit box, textarea and its read-only stand-in while dictating (54 §7). */
const BOX =
  'min-h-10 resize-none border-0 bg-transparent px-2 py-2 text-[17px] leading-6 shadow-none focus-visible:ring-0 md:text-[17px] dark:bg-transparent'
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

/** The slash popup: commands matching the typed word, one highlighted. */
function SlashList({
  word,
  selected,
  onPick,
}: {
  word: string
  selected: number
  onPick(name: string): void
}) {
  return (
    <div className="absolute bottom-full left-0 mb-2 max-h-72 w-full overflow-y-auto rounded-xl border bg-popover p-1 text-sm shadow-md">
      {matches(word).map((c, i) => (
        <button
          type="button"
          key={c.name}
          onMouseDown={(e) => e.preventDefault()}
          onClick={() => onPick(c.name)}
          className={cn(
            'flex w-full items-baseline gap-2 rounded-lg px-3 py-2 text-left',
            i === selected && 'bg-accent',
          )}
        >
          <span className="font-mono">{c.name}</span>
          {c.hint && <span className="font-mono text-fg-tertiary">{c.hint}</span>}
          <span className="ml-auto truncate text-fg-secondary">{c.desc}</span>
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
  compact = false,
  readOnly = false,
}: {
  session: string | null
  /** The split pane this composer sits in when it is not `session`'s (a draft pane's id); `null` = not tiled, always addressed. */
  pane?: string | null
  draftCreate?: DraftCreate
  /** 59 §7.3 chat under a note: this document's marker line leads every message (not removable here). */
  pinned?: AttachRef
  /** Title of the session the first message creates (a note's chat: the note title). */
  sessionTitle?: string
  /** The first message created `session`; the host keeps it instead of the default navigation to `/c/$session`. */
  onCreated?: (session: string) => void
  /** Tighter box for a panel. */
  compact?: boolean
  /** 76 §0.12: the talk view's transcript — the box shows but takes nothing; speech is the input. */
  readOnly?: boolean
}) {
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
  const defaultModel = useConnection((s) => s.info?.default_model)
  // 83 §3: a new session starts on the engine's default effort (a draft pick set it via `set_effort(null, …)`).
  const defaultEffort = useConnection((s) => s.info?.default_effort)
  const { data: models } = useQuery({
    queryKey: queryKeys.models,
    queryFn: () => api.list_models(),
  })
  // 52 §3.2: what happens to an image the main model cannot see; the server stays authoritative when this is stale.
  const { data: visionModel } = useQuery({
    queryKey: queryKeys.visionModel,
    queryFn: () => api.vision_model(),
  })
  // The skill index: which `/<word>` sends are skills (`isCommand`); the popup itself lists only `COMMANDS`.
  const { data: skillIndex } = useQuery({
    queryKey: queryKeys.skills,
    queryFn: () => api.list_skills(),
  })
  // A panel's composer (`onCreated`) keeps its own draft apart from the chat page's for the same session.
  // The draft is the store's (`lib/drafts`): durable across remounts, pane switches and a relaunch.
  const draftKey = `${onCreated ? 'panel:' : ''}${session ?? ''}`
  const { text, pastes, unsent } = useDrafts((s) => s.by[draftKey] ?? EMPTY)
  const setText = (t: string) => useDrafts.getState().setText(draftKey, t)
  const setPastes = (p: Pastes | ((held: Pastes) => Pastes)) => useDrafts.getState().setPastes(draftKey, p)
  /** Skill parked for the next send (`lib/skill-draft`): the send becomes `use_skill(session, skill, words)`. */
  const skill = useSkillDraft((s) => (session ? s.by[session] : undefined))
  const setSkill = (name: string | null) => session && useSkillDraft.getState().set(session, name)
  const [selected, setSelected] = useState(0)
  const [busy, setBusy] = useState(false)
  /** 68 §5: a send the owner could not take; the card above the composer offers to take the session here and resend. */
  const [offline, setOffline] = useState<OwnerUnreachable | null>(null)
  const voice = speech.useVoice()
  const recorder = useRef<{ rec: MediaRecorder; discard: boolean } | null>(null)
  /** A release (send / cancel) that beat `getUserMedia`: applied the moment the recorder exists. */
  const pendingStop = useRef<boolean | null>(null)
  const [recording, setRecording] = useState(false)
  const [secs, setSecs] = useState(0)
  /** 82 §C: the recorder stream's analyser on its own context; the wave the box shows while `recording`. */
  const noteWave = useRef<{ dispose: () => void } | null>(null)
  const [noteSource, setNoteSource] = useState<FrameSource<VisualFrame> | null>(null)
  const dropNoteWave = () => {
    noteWave.current?.dispose()
    noteWave.current = null
    setNoteSource(null)
  }
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
  /** Caret to place once the textarea is back after a dictation. */
  const [pendingCaret, setPendingCaret] = useState<number | null>(null)
  const { state: comm, exports } = useCommunication(session)
  // Owned private tree (51 §4.5): uploads go to its scratch root; a private draft has no root yet, so blob uploads
  // (which would land in the durable store) and voice notes (transcribed from the durable store) wait for the session.
  const privateRoot = usePrivate((s) => s.current?.root)
  const priv = session ? privateRoot !== undefined && isPrivate(session) : (draftCreate?.private ?? false)
  const noUpload = priv && !session

  const slashWord = /^\/\S*$/.test(text) ? text : null
  const slash = slashWord !== null && matches(slashWord).length > 0
  const held = pastes.size > 0
  /** The attach picker (66 §7.1): `'menu'` from `+`/`theuth:attach-open`, `'at'` from the `@` trigger (§7.8). */
  const [picking, setPicking] = useState<'menu' | 'at' | null>(null)
  const [menu, setMenu] = useState(false)
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
    void dictation.start(api, text.slice(0, at), text.slice(at), useEngines.getState().active !== LOCAL)
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
  useEffect(
    () =>
      dictation.onFinish(({ value, caret }) => {
        if (!ownerRef.current) return
        setText(value)
        setPendingCaret(caret)
      }),
    [],
  )
  // Idle again (finished, discarded, or `start` failed) → nobody owns it. `start` flips the phase synchronously,
  // so the same batch that sets `owner` never sees idle.
  useEffect(() => {
    if (dict.phase === 'idle') setOwner(false)
  }, [dict.phase])
  useEffect(() => {
    const ta = ref.current
    if (pendingCaret === null || !ta) return
    ta.setSelectionRange(pendingCaret, pendingCaret)
    ta.focus()
    setPendingCaret(null)
  }, [pendingCaret, dictating])

  /** Put `s` at the selection (replacing it), caret after it; a chip whose token the selection took goes too. */
  const insert = (s: string, held: Pastes = pastes) => {
    const ta = ref.current
    const [a, b] = ta ? [ta.selectionStart, ta.selectionEnd] : [text.length, text.length]
    const next = text.slice(0, a) + s + text.slice(b)
    setText(next)
    setPastes(reconcile(next, held))
    setPendingCaret(a + s.length)
  }

  const addFiles = (paths: string[], dims?: [number, number]) => {
    setPastes(paths.reduce((held, path) => attach(path, dims?.[0] ?? 0, dims?.[1] ?? 0, held), pastes))
    const id = model ?? defaultModel
    const info = models?.find((m) => m.id === id)
    if (info && !info.image && paths.some((p) => IMAGE.test(p)))
      toast.warning(
        visionModel
          ? `${info.name} does not accept images; ${visionModel.id} will inspect them`
          : `${info.name} does not accept images; they will be omitted unless a vision fallback is set (/vision)`,
      )
  }

  /** 66 §7.6: attach a reference; its chip stands above the box until send expands it to the marker line. */
  const addRef = (ref: AttachRef) => setPastes((held) => attachRef(ref, held))
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
    const held = refs.reduce((h, r) => attachRef(r, h), pastes)
    if (picking === 'at' && atWord) {
      const next = stripAt(text, atWord.start, atWord.word)
      setText(next)
      setPastes(reconcile(next, held))
      setPendingCaret(atWord.start)
    } else setPastes(held)
    setPicking(null)
    setAtWord(null)
    ref.current?.focus()
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
  const pathChanged = (next: string, caret: number) => {
    const t = pathTrigger(next, caret, pathWindows.current)
    const slashNow = /^\/\S*$/.test(next) && matches(next).length > 0
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
    setText(next.text)
    setPastes(reconcile(next.text, pastes))
    setPendingCaret(next.caret)
    setPath(null)
    setPathAsk(item.is_dir ? { text: next.text, caret: next.caret, now: true } : null)
  }
  /** The list is for the draft as it stands: a send or a recall (programmatic text) leaves no stale rows. */
  const pathOpen = path !== null && picking === null && pathTrigger(text, path.end, pathWindows.current)?.start === path.start

  // Uploads `blobs` through `put_attachment` (31 §9) — or `put_session_attachment` for a private tree — then attaches the engine paths; image dims for the first image.
  const addBlobs = async (blobs: (File | Blob)[]) => {
    if (blobs.length === 0) return
    if (noUpload) {
      toast.error('Send the first message of a private session before attaching files')
      return
    }
    try {
      let dims: [number, number] | undefined
      const first = blobs[0]
      if (first?.type.startsWith('image/')) {
        const bitmap = await createImageBitmap(first)
        dims = [bitmap.width, bitmap.height]
        bitmap.close()
      }
      addFiles(
        await Promise.all(blobs.map((b) => putAttachment(api, b, extFor(b), priv && session ? session : undefined))),
        dims,
      )
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    }
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
    void addBlobs([...e.dataTransfer.files])
  }

  const onPaste = async (e: ClipboardEvent<HTMLTextAreaElement>) => {
    const image = [...e.clipboardData.items].find((i) => i.type.startsWith('image/'))?.getAsFile()
    if (image) {
      e.preventDefault()
      await addBlobs([image])
      return
    }
    const plain = e.clipboardData.getData('text/plain')
    if (!plain) return
    e.preventDefault()
    const held = tokenize(plain, pastes)
    insert(held.text, held.pastes)
  }

  const reset = () => {
    setText('')
    setPastes(NONE)
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
   * `interject` (⌘/Ctrl+Enter) mid-turn: cancel the turn, then queue; the queued message starts the next turn now.
   * The box keeps its text until the engine acks (70 SEND-NEVER-LOSES): a failure leaves it flagged `unsent` with the
   * banner, and the retry (Retry, or Enter again) reuses the client input id so the engine never takes it twice. */
  const submit = async (spoken?: string, interject = false) => {
    const line = spoken ?? text.trim()
    if ((!line && !skill && !held) || busy) return
    setBusy(true)
    const mode = (id: string) => speech.setVoiceMode(id, spoken !== undefined)
    const clientId = unsent ?? crypto.randomUUID()
    /** The session the input went to (a new chat's once created), and how — for the rollback in `catch`. */
    let target: string | null = session
    let steered = false
    let body = ''
    try {
      if (line.startsWith('/skill:')) {
        const [name = '', ...words] = line.slice('/skill:'.length).trim().split(/\s+/)
        if (!name) throw new Error('usage: /skill:<name>')
        // The words stay in the box: `reset()` then `setText` when this composer stays mounted; a new chat's
        // composer reads them from `drafts` as it mounts.
        reset()
        await draftSkill(api, navigate, session, name, words.join(' '))
        if (session) setText(words.join(' '))
        return
      }
      if (isCommand(line, skillIndex?.map((s) => s.name) ?? null)) {
        reset()
        await run(line, ctx)
        return
      }
      // A parked skill: the server builds the `<skill>` input from its body and these words; `TurnStarted`
      // (idle) or `InputQueued` (running) echoes it as the skill card, so nothing is marked sent here.
      if (session && skill) {
        reset()
        setSkill(null)
        mode(session)
        if (running && interject) await api.cancel(session)
        await api.use_skill(session, skill, expand(line, pastes).text)
        return
      }
      const expanded = expand(line, pastes)
      // 59 §7.3: the pinned document leads every message, so its id survives compaction.
      body = pinned ? compose([pinned], expanded.text) : expanded.text
      const { files } = expanded
      const { markSent, applyEvents, loadHistory } = useSessions.getState()
      // 66 §7.6: recents are recorded on send, not on pick.
      const sent = [...pastes.values()].flatMap((p) => (p.kind === 'ref' ? [p.ref] : []))
      if (sent.length > 0) saveRecents(active, pushRecents(loadRecents(active), sent))
      // A voice note has no text in the box: park the line there so a failure has something to restore.
      if (spoken && !text) setText(spoken)
      useDrafts.getState().sending(draftKey, clientId)
      if (session && running) {
        steered = true
        applyEvents([{ type: 'InputQueued', session, id: clientId, text: body, defer: false }])
        mode(session)
        // Cancel first: its epoch bump would stale a steer admitted before it (todo auto-continue).
        if (interject) await api.cancel(session)
        await deadline(api.steer(session, body, clientId, files))
      } else if (session) {
        markSent(session, body, files)
        mode(session)
        await deadline(api.send_input(session, body, files, null, clientId))
      } else {
        let id: string
        if (draftCreate?.private || draftCreate?.project) {
          if (draftCreate.private && !usePrivate.getState().acknowledged)
            throw new Error('Acknowledge the private session notice first')
          const open = await api.create_session({
            retention: draftCreate.private ? 'private' : 'durable',
            project: draftCreate.project,
            view: draftCreate.private ? VIEW : null,
            cwd: null,
            model: null,
            effort: null,
            parent_session: null,
            parent_engine: null,
            harness: null,
          })
          if (open.retention === 'private') usePrivate.getState().open(open)
          id = open.session
        } else id = await api.new_session()
        target = id
        if (sessionTitle) await api.rename_session(id, sessionTitle)
        // Seed before sending: a fast failure can finish before the RPC reply.
        loadHistory(
          id,
          { ...FRESH, model: defaultModel ?? '', effort: defaultEffort ?? null },
          'replace',
        )
        markSent(id, body, files)
        mode(id)
        await deadline(api.send_input(id, body, files, null, clientId))
      }
      useDrafts.getState().acked(draftKey, spoken ?? text)
      setSelected(0)
      // A retry the engine already had starts no turn, so the row shown at once would stay a ghost: drop it. A
      // routed retry is either stamped by its `TurnStarted` already (then this is a no-op) or echoed by it after.
      // A steer's chip self-heals: the engine's `InputInjected` for the same id removes it.
      if (unsent && target && !steered) useSessions.getState().unmarkSent(target, body)
      if (target && !session) {
        if (onCreated) onCreated(target)
        else await navigate({ to: '/c/$session', params: { session: target } })
      }
    } catch (e) {
      // The row shown at once comes back off; the text is still in the box, flagged `unsent` for the banner.
      const st = useSessions.getState()
      if (target && steered) st.applyEvents([{ type: 'InputWithdrawn', session: target, id: clientId }])
      else if (target) st.unmarkSent(target, body)
      if (target && !session) {
        // The chat exists now: the unsent draft follows it, so Retry lands in that session.
        const drafts = useDrafts.getState()
        drafts.set(`${onCreated ? 'panel:' : ''}${target}`, drafts.by[draftKey] ?? EMPTY)
        drafts.set(draftKey, EMPTY)
        if (onCreated) onCreated(target)
        else await navigate({ to: '/c/$session', params: { session: target } })
      }
      if (e instanceof SendTimeout) useConnection.getState().client?.probe()
      const unreachable = ownerUnreachable(e)
      if (unreachable && target) setOffline(unreachable)
      else toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  /** Item 8: ⌘↩ / a long press with an empty box while steers are queued — cancel the turn and deliver them now,
   * in order, as the next input. Withdraw before cancel: cancel's epoch bump would stale them where they stand. */
  const flush = async () => {
    if (!session || !queued?.length || busy) return
    setBusy(true)
    try {
      const texts: string[] = []
      for (const q of queued) texts.push(await api.withdraw(session, q.id))
      await api.cancel(session)
      const { applyEvents } = useSessions.getState()
      for (const body of texts) {
        const id = crypto.randomUUID()
        applyEvents([{ type: 'InputQueued', session, id, text: body, defer: false }])
        await api.steer(session, body, id, [])
      }
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  // The composer's phase and its two gestures (`lib/composer-state`): Enter / the button, and ⌘↩ / a long press.
  const filled = Boolean(text.trim() || skill || held)
  const at = phase(Boolean(session && running), queued?.length ?? 0)
  const gesture = (g: 'press' | 'interrupt') => {
    const a = g === 'press' ? press(at, filled) : interrupt(at, filled)
    if (a === 'flush') void flush()
    else if (a) void submit(undefined, a === 'interject')
  }
  const steerPress = useLongPress(
    () => gesture('press'),
    () => gesture('interrupt'),
  )

  // Mic (45 §7.2): click to record, click again to upload → `transcribe` → send as `[voice note, N s] …`; Escape discards.
  const stopRecording = (discard: boolean) => {
    const r = recorder.current
    if (!r) {
      if (pendingStop.current === null || discard) pendingStop.current = discard
      return
    }
    r.discard = discard
    r.rec.stop()
  }

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

  const record = async () => {
    // A user gesture: the one moment iOS lets the reply's `AudioContext` start (45 §7.3). Released on `onstop`
    // or a failed start, so an idle session does not keep iOS's Now Playing up.
    speech.unlock('record')
    pendingStop.current = null
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true })
      const rec = new MediaRecorder(stream, {
        mimeType: AUDIO_MIMES.find((m) => MediaRecorder.isTypeSupported(m)) ?? '',
        audioBitsPerSecond: 32_000,
      })
      const chunks: Blob[] = []
      rec.ondataavailable = (e) => chunks.push(e.data)
      let startedAt = 0
      rec.onstop = () => {
        dropNoteWave()
        for (const track of stream.getTracks()) track.stop()
        void speech.release('record')
        const discard = recorder.current?.discard ?? true
        recorder.current = null
        setRecording(false)
        if (discard) return
        // 89 VOICE-HOLD: a release within `MIN_NOTE_MS` of the start is a slip, not a message — no upload.
        if (performance.now() - startedAt < MIN_NOTE_MS) toast('Hold to record, release to send', { duration: 2000 })
        else void sendNote(new Blob(chunks, { type: rec.mimeType }))
      }
      // 82 §C: a dedicated context for the wave (speech.ts owns the audio session); closed with the recording.
      const actx = new AudioContext()
      const wave = analyserSource(actx, stream)
      noteWave.current = {
        dispose: () => {
          wave.dispose()
          void actx.close()
        },
      }
      recorder.current = { rec, discard: false }
      startedAt = performance.now()
      rec.start()
      setNoteSource(wave.source)
      setRecording(true)
      // The finger already lifted (a hold released before the mic opened): stop with what was said, or discard.
      const early = pendingStop.current
      if (early !== null) {
        pendingStop.current = null
        stopRecording(early)
      }
    } catch (e) {
      pendingStop.current = null
      void speech.release('record')
      toast.error(e instanceof Error ? e.message : String(e))
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

  const pick = (name: string) => {
    const hint = matches(name)[0]?.hint ?? ''
    setText(`${name} `)
    setSelected(0)
    if (hint === '') {
      reset()
      void run(name, ctx).catch((e: unknown) => toast.error(e instanceof Error ? e.message : String(e)))
    }
  }

  const onKeyDown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (slash && slashWord) {
      const list = matches(slashWord)
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault()
        setSelected((i) => (i + (e.key === 'ArrowDown' ? 1 : list.length - 1)) % list.length)
        return
      }
      if (e.key === 'Tab' || (e.key === 'Enter' && !e.shiftKey)) {
        e.preventDefault()
        const c = list[selected] ?? list[0]
        if (c) pick(c.name)
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
      void api.cancel(session)
      return
    }
    if (session && queued?.length && chord(keys.withdraw, k)) {
      e.preventDefault()
      const last = queued[queued.length - 1]
      if (last) void api.withdraw(session, last.id).then(setText, (err: unknown) => toast.error(err instanceof Error ? err.message : String(err)))
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

  return (
    <div className={cn('mx-auto w-full max-w-[768px]', compact ? 'px-2 pb-1' : 'px-4 pb-2')}>
      {!compact && <CommunicationNotice session={session} state={comm} exports={exports} />}
      {session && offline && (
        <OwnerOfflineCard session={session} offline={offline} onDone={() => setOffline(null)} onResend={() => submit()} />
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
      {session && <QueuedChips session={session} onEdit={(t) => setText(t)} />}
      {skill && (
        <div className="flex justify-end pb-2">
          <span className="flex items-center gap-1.5 rounded-full bg-layer-2 px-3 py-1 text-sm text-fg-secondary">
            <Sparkles className="size-3.5" />
            <span className="font-medium">{skill}</span>
            <button type="button" aria-label="Remove skill" onClick={() => setSkill(null)} className="ml-1 hover:text-foreground">
              <X className="size-3" />
            </button>
          </span>
        </div>
      )}
      {session && !compact && <ContextPicker session={session} />}
      <input
        ref={fileInput}
        type="file"
        accept="image/*,application/pdf"
        multiple
        className="hidden"
        onChange={(e) => {
          void addBlobs([...(e.target.files ?? [])])
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
      <div
        ref={box}
        className="relative rounded-[28px] border border-border-subtle bg-elevated px-2 py-[7px] shadow-sm"
        onDragOver={(e) => e.preventDefault()}
        onDrop={onDrop}
      >
        {slash && slashWord && <SlashList word={slashWord} selected={selected} onPick={pick} />}
        {path && pathOpen && <PathList popup={path} mobile={isMobile} onPick={pickPath} />}
        <HeldStrip
          pastes={pastes}
          pinned={pinned}
          onChange={setPastes}
          onRemove={(i) => {
            setText(untoken(text, i))
            setPastes(drop(i, pastes))
          }}
        />
        <div className="flex items-end gap-1">
          {/* 89 VOICE-HOLD: a held or locked note takes the row — Delete (locked) · dot + timer stand in for Add. */}
          {stage === 'locked' && <DeleteNote onClick={() => stopRecording(true)} />}
          {stage !== 'idle' && <RecordingMeta secs={secs} stage={stage} />}
          <Popover open={menu} onOpenChange={setMenu}>
            <PopoverTrigger asChild>
              <Button
                variant="ghost"
                className={cn('size-9 shrink-0 rounded-full text-fg-secondary', stage !== 'idle' && 'hidden')}
                aria-label="Add"
              >
                <Plus className="size-5" />
              </Button>
            </PopoverTrigger>
            <PopoverContent align="start" className="w-52 p-1">
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
              <Button variant="ghost" className="w-full justify-start" onClick={() => openBroadcastDialog({ text })}>
                <Megaphone /> Send to many…
              </Button>
            </PopoverContent>
          </Popover>
          <div className="relative min-w-0 flex-1">
            {/* 82 §C: while a voice note records the wave is the box; the textarea stays mounted underneath. */}
            <RecordingBox recording={recording} source={noteSource}>
              {dictating && dict.active ? (
                // 94 DICTATION-LAYOUT: text on top at full width, the voice bar (wave + stop) spread underneath.
                <div className="flex flex-col">
                  <DictatedText
                    before={dict.active.before}
                    after={dict.active.after}
                    utts={dict.active.utts}
                    className={cn(BOX, touch() ? 'max-h-[160px]' : 'max-h-[200px]')}
                  />
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <button
                        type="button"
                        className="mt-1 flex h-9 w-full items-center overflow-hidden rounded-full bg-fg-accent/10 px-1 text-fg-secondary hover:bg-fg-accent/15"
                        aria-label="Stop dictation"
                        onClick={dictate}
                      >
                        <VoiceWave state="listening" source={dictSource} label="Dictation level" className="h-full flex-1" />
                        <Square className="size-3.5 shrink-0 fill-current" />
                      </button>
                    </TooltipTrigger>
                    <TooltipContent>Stop dictation (Ctrl+D)</TooltipContent>
                  </Tooltip>
                </div>
              ) : (
                <Textarea
                  readOnly={readOnly}
                  dictate={false}
                  ref={ref}
                  value={text}
                  onChange={(e) => {
                    setText(e.target.value)
                    setPastes(reconcile(e.target.value, pastes))
                    setSelected(0)
                    changed(e.target.value, e.target.selectionStart)
                    pathChanged(e.target.value, e.target.selectionStart)
                  }}
                  onKeyDown={onKeyDown}
                  onPaste={onPaste}
                  rows={1}
                  // An exported session's human input never joins a peer task; it steers a human turn or waits behind a task (50 §6.3).
                  placeholder={readOnly ? 'Talking — speak, or stop the talk to type' : running ? (exports.length ? 'Send privately…' : 'Send to steer…') : skill ? `Words for ${skill} (optional)` : 'Ask anything'}
                  // ponytail: `field-sizing: content` grows the box (6 lines on touch, 8 on desktop, then scrolls); older WebKit keeps one line.
                  className={cn(BOX, touch() ? 'max-h-[160px]' : 'max-h-[200px]')}
                />
              )}
            </RecordingBox>
            {(stage === 'held' || stage === 'cancelled') && <CancelHint ref={hint} />}
          </div>
          {/* 76 §0.12: the former voice-note button is Talk — the live spoken conversation on this session. B3: while it
              runs here, the chat view keeps a compact indicator: tap opens the talk view, the x ends the talk. */}
          {talkHere ? (
            <div className="flex shrink-0 items-center rounded-full bg-fg-accent/10">
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
            (at !== 'idle' && session ? (
              <>
                {/* Item 7: a finger's steer. Tap queues the box into the running turn; hold interrupts (with the box,
                    or the queued steers when it is empty). Not `disabled`: an empty box still takes the hold. */}
                {(filled || at === 'queued') && (
                  <Tooltip>
                    <TooltipTrigger asChild>
                      <Button
                        className={cn(
                          'size-9 shrink-0 rounded-full bg-fg-accent text-primary-foreground hover:bg-fg-accent-hover select-none touch-none',
                          (!filled || busy || dictating || readOnly) && 'opacity-50',
                        )}
                        aria-label="Steer"
                        aria-disabled={!filled || busy || dictating || readOnly}
                        {...steerPress}
                      >
                        <ArrowUp className="size-5" />
                      </Button>
                    </TooltipTrigger>
                    <TooltipContent>
                      {touch() ? 'Steer · hold to interrupt' : `Steer (${chordLabel(keys.send)}) · hold or ${chordLabel(keys.interject)} to interrupt`}
                    </TooltipContent>
                  </Tooltip>
                )}
                <Button
                  className="size-9 shrink-0 rounded-full bg-danger text-primary-foreground hover:bg-danger/90"
                  aria-label="Stop"
                  onClick={() => void api.cancel(session)}
                >
                  <Square className="size-4 fill-current" />
                </Button>
              </>
            ) : (
              <Button
                className="size-9 shrink-0 rounded-full bg-fg-accent text-primary-foreground hover:bg-fg-accent-hover"
                aria-label="Send"
                disabled={!filled || busy || dictating || readOnly}
                onClick={() => gesture('press')}
              >
                <ArrowUp className="size-5" />
              </Button>
            ))}
        </div>
      </div>
    </div>
  )
}

/**
 * The read-only box while dictating (54 §5.1): `before`, a separating space when `before` does not end in one,
 * each utterance (muted until the VAD closes it), a space when `after` does not start with one or punctuation, `after`.
 * The spacing rules mirror `splice()`; the caret is hidden and nothing here takes input.
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
  const spoken = utts.filter((u) => u.text !== '')
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
          <span className={cn('transition-colors duration-150', !u.final && 'text-fg-tertiary')}>{u.text}</span>
        </Fragment>
      ))}
      {trail}
      {after}
    </div>
  )
}
