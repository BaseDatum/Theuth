import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { open as pickFiles } from '@tauri-apps/plugin-dialog'
import { ArrowUp, AudioLines, FileUp, Megaphone, Mic, Paperclip, Plus, Sparkles, Square, StickyNote, X } from 'lucide-react'
import {
  type ClipboardEvent,
  type DragEvent,
  Fragment,
  type KeyboardEvent,
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
import { ownerUnreachable } from '@/lib/owner'
import { VIEW, isPrivate, usePrivate } from '@/lib/private'
import { queryKeys, useApi } from '@/lib/rpc'
import { drafts, useSkillDraft } from '@/lib/drafts'
import * as speech from '@/lib/speech'
import { matches as chord } from '@/lib/chord'
import { useShortcuts } from '@/lib/shortcuts'
import { cn } from '@/lib/utils'
import { noteText } from '@/lib/voice-note'
import { LOCAL, useEngines } from '@/link/engines'
import { useConnection } from '@/store/connection'
import { useSessionField, useSessions } from '@/store/sessions'
import { createNote } from '@/components/notes/create'
import { useNotes } from '@/components/notes/useNotes'
import { AttachPicker } from './AttachPicker'
import { CommunicationNotice, useCommunication } from './CommunicationNotice'
import { openBroadcastDialog } from './BroadcastDialog'
import { ContextPicker } from './ContextPicker'
import { HeldStrip } from './HeldStrip'
import { type Offline, OwnerOfflineCard } from './OwnerOfflineCard'
import { LevelMeter } from './LevelMeter'
import { QueuedChips } from './QueuedChips'
import { type Ctx, FRESH, draftSkill, isCommand, matches, run } from './slash'

/** New Chat options (51 §8.3): `private` selects memory-only retention; `project` is root metadata. */
export type DraftCreate = { private: boolean; project: string | null }

const NONE: Pastes = new Map()
const IMAGE = /\.(png|jpe?g|gif|webp|bmp)$/i
/**
 * Seed the next mount of `session`'s composer (`null` = the new chat at `/`; `panel` = a note page's panel) with `text`
 * and a held chip per reference in `refs` (59 §5.4 "Ask theuth…", 66 §7.8 "Attach to chat…").
 */
export function prefillDraft(session: string | null, text: string, refs: AttachRef[] = [], panel = false): void {
  drafts.set(`${panel ? 'panel:' : ''}${session ?? ''}`, { text, pastes: refs.reduce((held, r) => attachRef(r, held), NONE) })
}
/** Recorder containers in preference order (45 §7.2): WebKit gives AAC in MP4, Chromium Opus in WebM; the engine decodes all. */
const AUDIO_MIMES = ['audio/mp4', 'audio/webm;codecs=opus', 'audio/ogg;codecs=opus', 'audio/webm']
/** The edit box, textarea and its read-only stand-in while dictating (54 §7). */
const BOX =
  'min-h-10 resize-none border-0 bg-transparent px-2 py-2 text-[17px] leading-6 shadow-none focus-visible:ring-0 md:text-[17px] dark:bg-transparent'

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
  draftEffort,
  draftCreate,
  pinned,
  sessionTitle,
  onCreated,
  compact = false,
}: {
  session: string | null
  /** The split pane this composer sits in when it is not `session`'s (a draft pane's id); `null` = not tiled, always addressed. */
  pane?: string | null
  draftEffort?: string | null
  draftCreate?: DraftCreate
  /** 59 §7.3 chat under a note: this document's marker line leads every message (not removable here). */
  pinned?: AttachRef
  /** Title of the session the first message creates (a note's chat: the note title). */
  sessionTitle?: string
  /** The first message created `session`; the host keeps it instead of the default navigation to `/c/$session`. */
  onCreated?: (session: string) => void
  /** Tighter box for a panel. */
  compact?: boolean
}) {
  const api = useApi()
  const active = useEngines((s) => s.active)
  // Host paths (native drop / dialog) only mean something to the daemon on this machine.
  const hostFiles = isDesktop() && active === LOCAL
  const fileInput = useRef<HTMLInputElement>(null)
  const navigate = useNavigate()
  const running = useSessionField(session ?? '', 'running') ?? false
  const queued = useSessionField(session ?? '', 'queued')
  const keys = useShortcuts((s) => s.chords)
  const model = useSessionField(session ?? '', 'model')
  const parent = useSessionField(session ?? '', 'parent')
  const defaultModel = useConnection((s) => s.info?.default_model)
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
  const draftKey = `${onCreated ? 'panel:' : ''}${session ?? ''}`
  const [text, setText] = useState(() => drafts.get(draftKey)?.text ?? '')
  const [pastes, setPastes] = useState<Pastes>(() => drafts.get(draftKey)?.pastes ?? NONE)
  useEffect(() => {
    drafts.set(draftKey, { text, pastes })
  }, [draftKey, text, pastes])
  /** Skill parked for the next send (`lib/skill-draft`): the send becomes `use_skill(session, skill, words)`. */
  const skill = useSkillDraft((s) => (session ? s.by[session] : undefined))
  const setSkill = (name: string | null) => session && useSkillDraft.getState().set(session, name)
  const [selected, setSelected] = useState(0)
  const [busy, setBusy] = useState(false)
  /** 68 §5: a send the owner could not take; the card above the composer offers to take the session here and resend. */
  const [offline, setOffline] = useState<Offline | null>(null)
  const voice = speech.useVoice()
  const recorder = useRef<{ rec: MediaRecorder; discard: boolean } | null>(null)
  const [recording, setRecording] = useState(false)
  const [secs, setSecs] = useState(0)
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
  const canDictate = voice && !priv
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
      if (e.key === 'Escape') void dictation.discard(api)
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

  // The macOS webview swallows HTML5 drops and reports host paths instead; one native listener for the component's life reads the latest handler through the ref.
  const dropPaths = (paths: string[]) => {
    if (hostFiles) addFiles(paths)
    else toast.error('Dropped files stay on this Mac; use Attach to upload them')
  }
  const dropRef = useRef(dropPaths)
  dropRef.current = dropPaths
  useEffect(() => {
    if (!isDesktop()) return
    const un = getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type === 'drop') dropRef.current(e.payload.paths)
    })
    return () => void un.then((f) => f())
  }, [])

  // 66 §7.8: a dragged row (`application/x-theuth-attach`) attaches; anything else is files.
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
   * `interject` (⌘/Ctrl+Enter) mid-turn: cancel the turn, then queue; the queued message starts the next turn now. */
  const submit = async (spoken?: string, interject = false) => {
    const line = spoken ?? text.trim()
    if ((!line && !skill && !held) || busy) return
    setBusy(true)
    const mode = (id: string) => speech.setVoiceMode(id, spoken !== undefined)
    // The input a refused send would resend after "Continue here" (68 §5).
    let pending: { body: string; files: string[] } | null = null
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
      const body = pinned ? compose([pinned], expanded.text) : expanded.text
      const { files } = expanded
      pending = { body, files }
      const { markSent, applyEvents, loadHistory } = useSessions.getState()
      // 66 §7.6: recents are recorded on send, not on pick.
      const sent = [...pastes.values()].flatMap((p) => (p.kind === 'ref' ? [p.ref] : []))
      if (sent.length > 0) saveRecents(active, pushRecents(loadRecents(active), sent))
      reset()
      if (session && running) {
        const id = crypto.randomUUID()
        applyEvents([{ type: 'InputQueued', session, id, text: body, defer: false }])
        mode(session)
        // Cancel first: its epoch bump would stale a steer admitted before it (todo auto-continue).
        if (interject) await api.cancel(session)
        await api.steer(session, body, id, files)
      } else if (session) {
        markSent(session, body, files)
        mode(session)
        await api.send_input(session, body, files)
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
        if (sessionTitle) await api.rename_session(id, sessionTitle)
        if (draftEffort !== undefined) await api.set_effort(id, draftEffort)
        // Seed before sending: a fast failure can finish before the RPC reply.
        loadHistory(
          id,
          { ...FRESH, model: defaultModel ?? '', effort: draftEffort ?? null },
          'replace',
        )
        markSent(id, body, files)
        mode(id)
        await api.send_input(id, body, files)
        if (onCreated) onCreated(id)
        else await navigate({ to: '/c/$session', params: { session: id } })
      }
    } catch (e) {
      const unreachable = ownerUnreachable(e)
      if (unreachable && session && pending) setOffline({ ...unreachable, ...pending })
      else toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(false)
    }
  }

  // Mic (45 §7.2): click to record, click again to upload → `transcribe` → send as `[voice note, N s] …`; Escape discards.
  const stopRecording = (discard: boolean) => {
    const r = recorder.current
    if (!r) return
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
    try {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true })
      const rec = new MediaRecorder(stream, {
        mimeType: AUDIO_MIMES.find((m) => MediaRecorder.isTypeSupported(m)) ?? '',
        audioBitsPerSecond: 32_000,
      })
      const chunks: Blob[] = []
      rec.ondataavailable = (e) => chunks.push(e.data)
      rec.onstop = () => {
        for (const track of stream.getTracks()) track.stop()
        void speech.release('record')
        const discard = recorder.current?.discard ?? true
        recorder.current = null
        setRecording(false)
        if (!discard) void sendNote(new Blob(chunks, { type: rec.mimeType }))
      }
      recorder.current = { rec, discard: false }
      rec.start()
      setRecording(true)
    } catch (e) {
      void speech.release('record')
      toast.error(e instanceof Error ? e.message : String(e))
    }
  }

  useEffect(() => {
    if (!recording) return
    setSecs(0)
    const tick = setInterval(() => setSecs((s) => s + 1), 1000)
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key === 'Escape') stopRecording(true)
    }
    window.addEventListener('keydown', onKey)
    return () => {
      clearInterval(tick)
      window.removeEventListener('keydown', onKey)
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
      void submit(undefined, true)
    } else if (chord(keys.send, k)) {
      e.preventDefault()
      void submit(undefined, false)
    }
  }

  return (
    <div className={cn('mx-auto w-full max-w-[768px]', compact ? 'px-2 pb-1' : 'px-4 pb-2')}>
      {!compact && <CommunicationNotice session={session} state={comm} exports={exports} />}
      {session && offline && <OwnerOfflineCard session={session} offline={offline} onDone={() => setOffline(null)} />}
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
          <Popover open={menu} onOpenChange={setMenu}>
            <PopoverTrigger asChild>
              <Button
                variant="ghost"
                className="size-9 shrink-0 rounded-full text-fg-secondary"
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
          <div className="min-w-0 flex-1">
            {dictating && dict.active ? (
              <DictatedText
                before={dict.active.before}
                after={dict.active.after}
                utts={dict.active.utts}
                className={cn(BOX, touch() ? 'max-h-[160px]' : 'max-h-[200px]')}
              />
            ) : (
              <Textarea
                ref={ref}
                value={text}
                onChange={(e) => {
                  setText(e.target.value)
                  setPastes(reconcile(e.target.value, pastes))
                  setSelected(0)
                  changed(e.target.value, e.target.selectionStart)
                }}
                onKeyDown={onKeyDown}
                onPaste={onPaste}
                rows={1}
                // An exported session's human input never joins a peer task; it steers a human turn or waits behind a task (50 §6.3).
                placeholder={running ? (exports.length ? 'Send privately…' : 'Send to steer…') : skill ? `Words for ${skill} (optional)` : 'Ask anything'}
                // ponytail: `field-sizing: content` grows the box (6 lines on touch, 8 on desktop, then scrolls); older WebKit keeps one line.
                className={cn(BOX, touch() ? 'max-h-[160px]' : 'max-h-[200px]')}
              />
            )}
          </div>
          {canDictate && !dictating && (recording || !text.trim()) && (
            <Tooltip>
              <TooltipTrigger asChild>
                <Button
                  variant={recording ? 'destructive' : 'ghost'}
                  className={cn('size-9 shrink-0 rounded-full', recording && 'w-auto gap-1 px-3 tabular-nums')}
                  aria-label={recording ? 'Stop recording' : 'Record voice note'}
                  disabled={busy || transcribing}
                  onClick={() => (recording ? stopRecording(false) : void record())}
                >
                  {recording ? (
                    <>
                      <Square className="size-4 fill-current" /> {secs} s
                    </>
                  ) : (
                    <AudioLines className="size-5" />
                  )}
                </Button>
              </TooltipTrigger>
              <TooltipContent>{running ? 'Voice note (steers the running turn)' : 'Voice note'}</TooltipContent>
            </Tooltip>
          )}
          {canDictate &&
            (dictating && dict.active ? (
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    className="h-9 w-24 shrink-0 rounded-full bg-fg-accent/10 p-0"
                    aria-label="Stop dictation"
                    onClick={dictate}
                  >
                    <LevelMeter levels={dict.active.levels} active />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>Stop dictation (Ctrl+D)</TooltipContent>
              </Tooltip>
            ) : (
              // `aria-disabled`, not `disabled`: a disabled button takes no pointer events and the tooltip could not say why.
              <Tooltip>
                <TooltipTrigger asChild>
                  <Button
                    variant="ghost"
                    className={cn('size-9 shrink-0 rounded-full', dictReason !== null && 'opacity-50')}
                    aria-label="Dictation"
                    aria-disabled={dictReason !== null}
                    disabled={busy || recording}
                    onClick={dictate}
                  >
                    <Mic className="size-5" />
                  </Button>
                </TooltipTrigger>
                <TooltipContent>{dictReason ?? 'Dictation (Ctrl+D)'}</TooltipContent>
              </Tooltip>
            ))}
          {running && session ? (
            <Button
              className="size-9 shrink-0 rounded-full bg-danger text-primary-foreground hover:bg-danger/90"
              aria-label="Stop"
              onClick={() => void api.cancel(session)}
            >
              <Square className="size-4 fill-current" />
            </Button>
          ) : (
            <Button
              className="size-9 shrink-0 rounded-full bg-fg-accent text-primary-foreground hover:bg-fg-accent-hover"
              aria-label="Send"
              disabled={(!text.trim() && !skill && !held) || busy || dictating}
              onClick={() => void submit()}
            >
              <ArrowUp className="size-5" />
            </Button>
          )}
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
