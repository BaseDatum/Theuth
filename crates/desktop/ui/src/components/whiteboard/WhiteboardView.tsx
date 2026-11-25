// 218 §8.8: the whiteboard — full window from the route (over sidebar and status bar), or inside a tile. Owns the view
// store, the CanvasHost, the lazy canvas, and the shell around it: pill, cluster, composer sheet, reply bubble, card
// sheet, outline view, the native Pencil bridge and the keyboard. Reads the document through `useBoard`, writes through
// `canvas/ops` and the canvas handle; RPCs through `wbRpc`.

import './glass.css'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { Check, ChevronRight, X } from 'lucide-react'
import { Suspense, lazy, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { useStore } from 'zustand'
import { useNoteTitle } from '@/components/notes/NotePage'
import { useDocument } from '@/components/editor/useDocument'
import { useIsMobile } from '@/hooks/use-mobile'
import { extFor, putAttachment } from '@/lib/files'
import { noteText } from '@/lib/voice-note'
import { usePointerCoarse } from '@/lib/mobile'
import { notify } from '@/lib/notify'
import { queryKeys, useApi } from '@/lib/rpc'
import { useDark } from '@/lib/theme'
import type { EngineProvider } from '@/lib/doc/provider'
import { useQuerySupport } from '@/rpc/capabilities'
import { useConnection } from '@/store/connection'
import { useNotices } from '@/store/notices'
import { statusLine, useSessionField, useSessions } from '@/store/sessions'
import { useTerminals } from '@/components/chat/TerminalViewer'
import { MicButton } from '@/components/chat/MicButton'
import { useNoteRecorder } from '@/components/chat/use-note-recorder'
import { CancelHint, DeleteNote, RecordingMeta, type Stage } from '@/components/chat/VoiceHold'
import { type AddKind, Cluster } from './Cluster'
import { Bubble } from './Bubble'
import { CardSheet } from './CardSheet'
import { OutlineView, outlineText } from './OutlineView'
import { Pill, type SessionPick, useSessionTitle } from './Pill'
import { Sheet } from './Sheet'
import { asUrl, place, placeFromClipboard, placeImage } from './add'
import { bubbleAnchor, lastReply } from './reply'
import type { PlannedImage, SendPlan, WhiteboardCanvasHandle } from './canvas/api'
import type { Board, Id, Meta } from './canvas/model'
import * as ops from './canvas/ops'
import { createWhiteboardStore } from './canvas/store'
import { useBoard } from './canvas/useBoard'
import { clusterTimer } from './cluster-timer'
import { createHost } from './host'
import { type EdgeMark, FOLLOW_PAUSE_MS, edgeMarks } from './edges'
import { type NativeConnection, connectNativeInk } from './native-ink'
import { sendWhiteboard, withoutImage } from './send/send'
import { type WbTurnInfo, wbRpc } from './wire'

const Canvas = lazy(() => import('./canvas/Canvas'))

export type WhiteboardViewProps = {
  doc: string
  session?: string | null
  embedded?: boolean
  /** The target session changed (picked, created by a first send, detached). */
  onSession?(session: string | null): void
  onClose?(): void
}

export function WhiteboardView(props: WhiteboardViewProps) {
  const live = useDocument({ doc: props.doc })
  const frame = props.embedded ? 'relative size-full overflow-hidden bg-background' : 'fixed inset-x-0 top-0 z-50 h-band overflow-hidden bg-background'
  if (live.error || live.status.kind === 'closed')
    return (
      <div className={`${frame} grid place-items-center`}>
        <div className="flex flex-col items-center gap-3 text-sm text-fg-secondary">
          <p>{live.error?.message ?? 'This whiteboard could not be opened.'}</p>
          {props.onClose && (
            <button type="button" onClick={props.onClose} className="rounded-lg bg-layer-2 px-3 py-1.5">
              Back to whiteboards
            </button>
          )}
        </div>
      </div>
    )
  if (!live.provider) return <div className={`${frame} animate-pulse`} aria-busy aria-label="Opening the whiteboard" />
  return <Shell key={props.doc} {...props} provider={live.provider} frame={frame} />
}

type Chooser = { title: string; items: { key: string; label: string; detail?: string; run(): void }[] } | { title: string; input: string; placeholder: string; run(value: string): void }

const cardsOf = (b: Board) => Object.values(b.cards).filter((c) => c.state !== 'archived').length

/** The bound session's history, loaded once, so the reply bubble has its text. */
function useLoadedSession(session: string | null) {
  const api = useApi()
  const can = useQuerySupport(['load_session'])
  useEffect(() => {
    if (!session || !can) return
    const st = useSessions.getState()
    st.ensure(session)
    if (st.sessions[session]?.loaded) return
    api.load_session(session, null, 60).then(
      (page) => useSessions.getState().loadHistory(session, page, 'replace'),
      () => {},
    )
  }, [api, session, can])
}

function Shell({ doc, session: wanted = null, embedded = false, onSession, onClose, provider, frame }: WhiteboardViewProps & { provider: EngineProvider; frame: string }) {
  const api = useApi()
  const client = useConnection((s) => s.client)
  const qc = useQueryClient()
  const navigate = useNavigate()
  const dark = useDark()
  const phone = useIsMobile()
  const coarse = usePointerCoarse()
  const [store] = useState(() => createWhiteboardStore())
  const handle = useRef<WhiteboardCanvasHandle>(null)
  const rpc = useMemo(() => (client ? wbRpc(client) : null), [client])
  const rpcRef = useRef(rpc)
  rpcRef.current = rpc
  const host = useMemo(() => createHost({ doc, rpc: () => rpcRef.current, touch: coarse || phone }), [doc, coarse, phone])
  useEffect(() => () => host.dispose(), [host])
  const board = useBoard(provider)
  const [title, rename] = useNoteTitle(doc, provider)
  const readOnly = board.meta.v > 1
  const [outline, setOutline] = useState(phone)
  const [openCard, setOpenCard] = useState<Id | null>(null)
  const [chooser, setChooser] = useState<Chooser | null>(null)
  const fileInput = useRef<HTMLInputElement>(null)

  // ── Sessions: the target (R1: none → the first send makes one), the bound list (R12 chip, picker).
  const bound = useQuery({ enabled: !!rpc, queryKey: ['whiteboard', 'sessions', doc], queryFn: () => (rpc as NonNullable<typeof rpc>).sessions(doc), staleTime: 10_000 })
  const boundList = bound.data ?? []
  const [target, setTargetState] = useState<string | null>(wanted)
  const [explicitNone, setExplicitNone] = useState(false)
  useEffect(() => setTargetState(wanted), [wanted])
  const first = boundList[0]?.session ?? null
  const session = target ?? (explicitNone ? null : first)
  const setTarget = useCallback(
    (s: string | null) => {
      setTargetState(s)
      setExplicitNone(s === null)
      onSession?.(s)
    },
    [onSession],
  )
  useLoadedSession(session)
  const running = (useSessionField(session ?? '', 'running') ?? false) as boolean
  const activity = useSessionField(session ?? '', 'activity')
  const statusText = useSessionField(session ?? '', 'statusText')
  const sessionTitle = useSessionTitle(session, boundList)
  const refreshBound = () => void qc.invalidateQueries({ queryKey: ['whiteboard', 'sessions', doc] })
  const pick: SessionPick = {
    session,
    title: sessionTitle,
    bound: boundList,
    busy: running,
    onPick: (s) => {
      setTarget(s)
      if (rpc && !boundList.some((b) => b.session === s)) void rpc.bind(s, doc).then(refreshBound, (e: Error) => notify.error(e.message))
    },
    onNew: () => setTarget(null),
    onDetach: (s) => {
      if (rpc) void rpc.unbind(s, doc).then(refreshBound, (e: Error) => notify.error(e.message))
      if (s === session) setTarget(null)
    },
  }

  // ── Notices while full-window show in the pill (the status bar is covered).
  const notice = useNotices((s) => s.current)
  const [noticeShown, setNoticeShown] = useState<string | null>(null)
  useEffect(() => {
    if (embedded || !notice || notice.dismissed) return setNoticeShown(null)
    setNoticeShown(notice.text)
    const ms = notice.expiresAt ? Math.max(0, notice.expiresAt - Date.now()) : 8_000
    const t = setTimeout(() => setNoticeShown(null), ms)
    return () => clearTimeout(t)
  }, [notice, embedded])
  const working = running ? (statusLine(activity ?? 'thinking', statusText) ?? (activity === 'tool' ? 'working…' : activity === 'idle' || !activity ? 'thinking…' : `${activity}…`)) : null
  const status = noticeShown ?? working

  // ── Cluster: timer (B4) mirrored into the store (B3 reads it for the attribution panel).
  const timer = useMemo(() => clusterTimer({ onChange: (open) => store.getState().set({ clusterOpen: open }) }), [store])
  useEffect(() => () => timer.dispose(), [timer])
  const clusterOpen = useStore(store, (s) => s.clusterOpen)
  const keepControls = useStore(store, (s) => s.keepControls)
  const composerOpen = useStore(store, (s) => s.composerOpen)
  useEffect(() => timer.keep(keepControls), [timer, keepControls])
  useEffect(() => store.subscribe((s, p) => s.penDown && !p.penDown && timer.pen()), [store, timer])

  // ── Native Pencil (iPad): attach once; the handle is read lazily (the canvas is a lazy chunk).
  const native = useRef<NativeConnection | null>(null)
  const darkRef = useRef(dark)
  darkRef.current = dark
  useEffect(() => {
    if (readOnly) return
    let gone = false
    void connectNativeInk({ store, handle: () => handle.current, dark: () => darkRef.current, showTools: () => timer.show() }).then((c) => {
      if (gone) c?.dispose()
      else native.current = c
    })
    return () => {
      gone = true
      native.current?.dispose()
      native.current = null
    }
  }, [store, timer, readOnly])
  // Visibility and the theme (the native picker's colour for `ink` flips with it) both re-push the plugin's state.
  useEffect(() => native.current?.visible(!outline && !openCard), [outline, openCard, dark])

  // ── Composer: plan when opened, tray removals, the off-screen opt-in.
  const [text, setText] = useState('')
  const [plan, setPlan] = useState<SendPlan>({ marks: [], images: [], offScreen: 0 })
  const [offScreen, setOffScreen] = useState(false)
  const [sending, setSending] = useState(false)
  const [focusField, setFocusField] = useState(false)
  const replan = useCallback((off: boolean) => {
    const h = handle.current
    setPlan(h ? h.planSend({ offScreen: off }) : { marks: [], images: [], offScreen: 0 })
  }, [])
  const composeThumb = useCallback((i: PlannedImage) => handle.current?.composeImage(i, 160) ?? Promise.reject(new Error('no canvas')), [])
  const openComposer = (focus = true) => {
    timer.hide()
    store.getState().set({ composerOpen: true })
    setOffScreen(false)
    replan(false)
    setFocusField(focus)
  }
  const closeComposer = () => store.getState().set({ composerOpen: false })
  const can = useQuerySupport(['list_models', 'vision_model'])
  const models = useQuery({ enabled: can, queryKey: queryKeys.models, queryFn: () => api.list_models() }).data
  const visionModel = useQuery({ enabled: can, queryKey: queryKeys.visionModel, queryFn: () => api.vision_model() }).data
  const model = useSessionField(session ?? '', 'model') ?? useConnection.getState().info?.default_model
  const info = models?.find((m) => m.id === model)
  const noVision = !!info && !info.image && !visionModel

  // ── The reply bubble: after a turn of the target session ends.
  type BubbleState = { text: string; turn: WbTurnInfo | null; session: string }
  const [bubble, setBubble] = useState<BubbleState | null>(null)
  const [undoing, setUndoing] = useState(false)
  const wasRunning = useRef(running)
  useEffect(() => {
    const was = wasRunning.current
    wasRunning.current = running
    if (!session || !was || running) return
    const reply = lastReply(useSessions.getState().sessions[session]?.entries ?? [])
    if (!reply) return
    const s = session
    setBubble({ text: reply, turn: null, session: s })
    rpc?.turn(doc, s).then(
      (turn) => setBubble((b) => (b && b.session === s ? { ...b, turn } : b)),
      () => {},
    )
  }, [running, session, rpc, doc])
  const anchor = bubble ? bubbleAnchor(bubble.turn, board) : null

  // ── Off-screen work (design §4.5): placeholders and the cards the target session's latest turn touched, when out
  // of view, get an edge chevron; tap → pan there. The turn is re-read while the agent works (board changes, ~1 s).
  const [turnCards, setTurnCards] = useState<string[]>([])
  useEffect(() => {
    if (!rpc || !session) return setTurnCards([])
    const t = setTimeout(() => {
      rpc.turn(doc, session).then(
        (turn) => setTurnCards(turn && !turn.undone ? turn.cards : []),
        () => {},
      )
    }, 900)
    return () => clearTimeout(t)
  }, [rpc, doc, session, board, running])
  const work = useMemo(() => {
    const ids = new Set(turnCards)
    for (const c of Object.values(board.cards)) if (c.placeholder !== undefined && c.by !== 'owner') ids.add(c.id)
    return [...ids].filter((id) => board.cards[id] && board.cards[id]?.state !== 'archived')
  }, [turnCards, board])
  const [edges, setEdges] = useState<EdgeMark[]>([])
  const edgeFrame = useRef(0)
  const recomputeEdges = useCallback(() => {
    cancelAnimationFrame(edgeFrame.current)
    edgeFrame.current = requestAnimationFrame(() => {
      const h = handle.current
      if (!h || work.length === 0) return setEdges([])
      const frame = h.frameRect()
      const targets = work.flatMap((id) => {
        const r = h.screenRectOf(id)
        return r ? [{ id, rect: r }] : []
      })
      setEdges(edgeMarks(frame, targets))
    })
  }, [work])
  useEffect(() => {
    recomputeEdges()
    return () => cancelAnimationFrame(edgeFrame.current)
  }, [recomputeEdges])

  // ── Follow: any finger or mouse touch on the canvas turns it off for 60 s, then it comes back (design §4.5).
  const followPause = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => () => clearTimeout(followPause.current ?? undefined), [])
  useEffect(() => store.subscribe((s, p) => s.follow !== p.follow && clearTimeout(followPause.current ?? undefined)), [store])
  const pauseFollow = (e: React.PointerEvent) => {
    if (e.pointerType === 'pen' || !store.getState().follow) return
    store.getState().set({ follow: false })
    followPause.current = setTimeout(() => store.getState().set({ follow: true }), FOLLOW_PAUSE_MS)
  }
  const undoTurn = async () => {
    if (!bubble?.turn || !rpc) return
    setUndoing(true)
    try {
      const r = await rpc.undoTurn(doc, bubble.session, bubble.turn.turn)
      const skipped = r.skipped.map((s) => `${s.alias ?? s.id}: ${s.why}`)
      if (skipped.length) notify.warning(`Undid ${r.undone} ${r.undone === 1 ? 'change' : 'changes'}; kept ${skipped.length}`, { description: skipped.join('; ') })
      else notify.success(r.undone ? `Undid the agent's turn (${r.undone} ${r.undone === 1 ? 'change' : 'changes'})` : 'Nothing to undo')
      setBubble((b) => (b?.turn ? { ...b, turn: { ...b.turn, undone: true } } : b))
    } catch (e) {
      notify.error(e instanceof Error ? e.message : String(e))
    } finally {
      setUndoing(false)
    }
  }
  const openSession = (s: string | null = session) => s && void navigate({ to: '/c/$session', params: { session: s } })

  /** `spoken`: a voice note from the pill's mic — it goes with a fresh plan (the marks in view), the typed text stays. */
  const send = async (spoken?: string) => {
    const h = handle.current
    if (!h || !rpc || sending) return
    setSending(true)
    setBubble(null)
    try {
      const r = await sendWhiteboard(
        {
          handle: h,
          capture: host.capture,
          recognize: native.current ? native.current.recognize : undefined,
          wb: rpc,
          putAttachment: (blob, ext, s) => putAttachment(api, blob, ext, s),
          sendInput: (s, body, files, id) => api.send_input(s, body, files, null, id),
          steer: (s, body, id, files) => api.steer(s, body, id, files),
          dpr: globalThis.devicePixelRatio || 1,
        },
        {
          doc,
          title: title || 'Whiteboard',
          cards: cardsOf(board),
          text: spoken ?? text,
          session,
          running,
          bound: !!session && boundList.some((b) => b.session === session),
          vision: !noVision,
          selection: store.getState().selection,
          clientId: crypto.randomUUID(),
          plan: spoken !== undefined && !store.getState().composerOpen ? undefined : plan,
          frameOf: (c) => board.cards[c]?.frame,
        },
      )
      // A first send (R1) made the session: it becomes the target (and `?session=`); the engine's echo fills its rows.
      if (r.session && r.session !== session) {
        setTarget(r.session)
        useSessions.getState().ensure(r.session)
      }
      refreshBound()
      if (spoken === undefined) setText('')
      closeComposer()
    } catch (e) {
      notify.error(e instanceof Error ? e.message : String(e))
    } finally {
      setSending(false)
    }
  }

  // ── Hold-to-talk (89 VOICE-HOLD, the recorder shared with the chat composer): a voice note is transcribed and sent
  // with the whiteboard as `[voice note, N s] …`; a tap opens the sheet for system dictation.
  const [stage, setStage] = useState<Stage>('idle')
  const [secs, setSecs] = useState(0)
  const [transcribing, setTranscribing] = useState(false)
  const hint = useRef<HTMLSpanElement>(null)
  const voice = useNoteRecorder((blob) => {
    setTranscribing(true)
    void (async () => {
      try {
        const t = await api.transcribe(await putAttachment(api, blob, extFor(blob)))
        if (!t.text.trim()) notify('Nothing recognized')
        else await send(noteText(t.text, Number(t.duration_ms)))
      } catch (e) {
        notify.error(e instanceof Error ? e.message : String(e))
      } finally {
        setTranscribing(false)
      }
    })()
  })
  useEffect(() => {
    if (!voice.recording) return
    setSecs(0)
    const tick = setInterval(() => setSecs((s) => s + 1), 1000)
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return
      e.preventDefault()
      voice.stop(true)
    }
    window.addEventListener('keydown', onKey)
    return () => {
      clearInterval(tick)
      window.removeEventListener('keydown', onKey)
      voice.dropWave()
    }
    // biome-ignore lint/correctness/useExhaustiveDependencies: keyed on the recording flag, as the composer's
  }, [voice.recording])
  const mic = (
    <MicButton
      recording={voice.recording}
      source={voice.source}
      hint={hint}
      onStage={setStage}
      disabled={sending || transcribing}
      reason={null}
      onTap={() => openComposer(true)}
      onRecord={() => void voice.record()}
      onStop={voice.stop}
    />
  )
  const recordingRow =
    stage !== 'idle' ? (
      <>
        {(stage === 'held' || stage === 'cancelled') && <CancelHint ref={hint} />}
        {stage === 'locked' && <DeleteNote onClick={() => voice.stop(true)} />}
        <RecordingMeta secs={secs} stage={stage} />
      </>
    ) : transcribing ? (
      <span className="wb-muted truncate text-[14px]">Listening back…</span>
    ) : undefined

  // ── Add.
  const terminals = useTerminals(session ?? '')
  const spot = (w: number, h: number) => handle.current?.freeSpot(w, h) ?? { x: 0, y: 0 }
  const select = (id: Id | null) => id && store.getState().set({ selection: [id] })
  const add = async (kind: AddKind) => {
    const d = provider.doc
    try {
      switch (kind) {
        case 'sticky':
          return select(place(d, spot, 'sticky', 'Sticky', { t: 'sticky', text: '' }))
        case 'note': {
          const n = await api.doc_create('Whiteboard note', null, null)
          return select(place(d, spot, 'note', n.title || 'Note', { t: 'note', doc: n.id }))
        }
        case 'web':
          return setChooser({
            title: 'Add a web page',
            input: '',
            placeholder: 'https://…',
            run: (v) => {
              const url = asUrl(v)
              if (!url) return notify.warning('That is not a web address')
              select(place(d, spot, 'web', new URL(url).hostname, { t: 'web', url }))
            },
          })
        case 'image':
          return fileInput.current?.click()
        case 'clipboard': {
          const id = await placeFromClipboard(d, spot, host.blobPut)
          return id ? select(id) : notify('The clipboard is empty')
        }
        case 'diff':
          if (!session) return
          return select(place(d, spot, 'diff', 'Working tree', { t: 'diff', session, spec: { mode: 'worktree' }, as_of: Date.now() }))
        case 'terminal':
          if (!session) return
          return setChooser({
            title: 'Add a terminal',
            items: terminals.map((t) => ({ key: t.id, label: t.name ?? t.command, detail: t.state, run: () => select(place(d, spot, 'terminal', t.name ?? t.command, { t: 'terminal', session, job: t.id })) })),
          })
        case 'artifact': {
          if (!session) return
          const list = await api.list_artifacts(session, null)
          const pages = list.filter((a) => a.mime.startsWith('text/html') || a.mime.startsWith('image/'))
          return setChooser({
            title: 'Add an artifact',
            items: pages.map((a) => ({
              key: a.call_id,
              label: a.title || a.name,
              detail: a.mime.startsWith('image/') ? 'image' : 'page',
              run: () =>
                select(
                  a.mime.startsWith('image/')
                    ? place(d, spot, 'image', a.title || a.name, { t: 'image', session, call_id: a.call_id })
                    : place(d, spot, 'page', a.title || a.name, { t: 'page', session, call_id: a.call_id }),
                ),
            })),
          })
        }
      }
    } catch (e) {
      notify.error(e instanceof Error ? e.message : String(e))
    }
  }
  const addable: Record<AddKind, boolean> = { sticky: true, note: true, web: true, image: true, clipboard: true, artifact: !!session, diff: !!session, terminal: !!session && terminals.length > 0 }

  // ── Keyboard (desktop, full window): V P E L F N O, ⌘0, ⌘↵, Esc.
  useEffect(() => {
    if (embedded) return
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null
      if (t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName))) return
      const st = store.getState()
      const mod = e.metaKey || e.ctrlKey
      if (mod && e.key === '0') return e.preventDefault(), handle.current?.fit('all')
      if (mod && e.key === 'Enter') return e.preventDefault(), openComposer()
      if (mod || e.altKey) return
      const k = e.key.toLowerCase()
      if (e.key === 'Escape') {
        if (openCard) return setOpenCard(null)
        if (st.entered) return st.set({ entered: null })
        if (st.composerOpen) return closeComposer()
        return timer.hide()
      }
      const modes: Record<string, 'select' | 'draw' | 'erase' | 'link' | 'frame'> = { v: 'select', p: 'draw', e: 'erase', l: 'link', f: 'frame' }
      const m = modes[k]
      if (m) return st.set({ mode: st.mode === m ? 'idle' : m })
      if (k === 'n' && !readOnly) return void add('sticky')
      if (k === 'o') return setOutline((o) => !o)
    }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  })

  // Desktop: the cluster also appears when the pointer rests in the bottom-right quadrant.
  const rest = useRef<ReturnType<typeof setTimeout> | null>(null)
  const onPointerMove = (e: React.PointerEvent) => {
    if (e.pointerType !== 'mouse' || coarse) return
    const r = e.currentTarget.getBoundingClientRect()
    const inQuad = e.clientX > r.left + r.width / 2 && e.clientY > r.top + r.height / 2
    if (rest.current) clearTimeout(rest.current)
    rest.current = inQuad && !store.getState().clusterOpen ? setTimeout(() => timer.show(), 600) : null
    if (store.getState().clusterOpen) timer.activity()
  }

  const more = (
    <MoreMenu
      title={title}
      onRename={rename}
      meta={board.meta}
      onBg={(bg) => ops.setBg(provider.doc, bg)}
      store={store}
      archived={Object.values(board.cards).filter((c) => c.state === 'archived').map((c) => ({ id: c.id, title: c.title }))}
      onRestore={(id) => ops.restore(provider.doc, [id])}
      onCopy={() => void navigator.clipboard?.writeText(outlineText(board, title)).then(() => notify.success('Outline copied'), () => notify.error('Could not copy'))}
      onClose={onClose}
    />
  )

  const card = openCard ? board.cards[openCard] : undefined
  const bubbleNode = bubble ? (
    <Bubble
      text={bubble.text}
      canUndo={!!bubble.turn && !bubble.turn.undone && bubble.turn.writes > 0}
      undoing={undoing}
      onUndo={() => void undoTurn()}
      onDismiss={() => setBubble(null)}
      onOpenSession={() => openSession(bubble.session)}
      floating={!anchor || outline}
      beside={clusterOpen}
    />
  ) : null

  return (
    <div className={frame} data-whiteboard={doc} onPointerMove={onPointerMove}>
      {outline ? (
        <div className="absolute inset-0 overflow-y-auto">
          <OutlineView
            board={board}
            title={title}
            a={{
              onOpen: setOpenCard,
              onShow: (id) => {
                setOutline(false)
                requestAnimationFrame(() => handle.current?.focus(id, { animate: !matchMedia('(prefers-reduced-motion: reduce)').matches }))
              },
              onDim: readOnly ? undefined : (id) => ops.dim(provider.doc, [id], 'set aside'),
              onUndim: readOnly ? undefined : (id) => ops.patchCard(provider.doc, id, { state: 'live', tag: undefined }),
              onArchive: readOnly ? undefined : (id) => ops.archive(provider.doc, [id]),
              onRestore: readOnly ? undefined : (id) => ops.restore(provider.doc, [id]),
              onPin: readOnly ? undefined : (id, on) => ops.pinMark(provider.doc, id, on),
              toolbar: (
                <>
                  <ToolbarButton onClick={() => setOutline(false)}>Show canvas</ToolbarButton>
                  {!readOnly && <ToolbarButton onClick={() => void add('sticky')}>Add sticky</ToolbarButton>}
                  {!readOnly && <ToolbarButton onClick={() => void add('note')}>Add note</ToolbarButton>}
                  {!readOnly && <ToolbarButton onClick={() => void add('web')}>Add web page</ToolbarButton>}
                  {!readOnly && <ToolbarButton onClick={() => void add('image')}>Add image</ToolbarButton>}
                  {!readOnly && <ToolbarButton onClick={() => void add('clipboard')}>Paste</ToolbarButton>}
                  <ToolbarButton onClick={() => handle.current?.undo()}>Undo</ToolbarButton>
                  <ToolbarButton onClick={() => handle.current?.redo()}>Redo</ToolbarButton>
                  <ToolbarButton onClick={() => void navigator.clipboard?.writeText(outlineText(board, title)).then(() => notify.success('Outline copied'))}>Copy outline</ToolbarButton>
                  {onClose && <ToolbarButton onClick={onClose}>Close</ToolbarButton>}
                </>
              ),
            }}
          />
        </div>
      ) : null}
      {/* The canvas stays mounted under the outline (its handle feeds the composer and the native bridge). */}
      <div className={outline ? 'invisible absolute inset-0' : 'absolute inset-0'} aria-hidden={outline} onPointerDownCapture={pauseFollow}>
        <Suspense fallback={<div className="size-full bg-background" />}>
          <Canvas
            ref={handle}
            provider={provider}
            store={store}
            host={host}
            readOnly={readOnly || phone}
            embedded={embedded}
            className="size-full"
            anchored={bubble && anchor && !outline ? [{ key: 'reply', card: anchor, node: bubbleNode }] : []}
            onCanvasTap={() => {
              if (store.getState().composerOpen) return closeComposer()
              timer.tap()
            }}
            onPenActivity={(down) => down && timer.pen()}
            onViewChange={(view, phase) => {
              recomputeEdges()
              if (phase === 'end') native.current?.viewEnd(view, handle.current?.frameRect() ?? new DOMRect())
            }}
            onCardOpen={setOpenCard}
          />
        </Suspense>
      </div>
      {!outline &&
        edges.map((m) => {
          const c = board.cards[m.id]
          return (
            <button
              key={m.id}
              type="button"
              aria-label={`Off screen: ${c?.title || 'a card'}${c?.placeholder !== undefined ? ' (being made)' : ''}. Show it`}
              title={c?.title}
              onClick={() => handle.current?.focus(m.id, { animate: !matchMedia('(prefers-reduced-motion: reduce)').matches })}
              className="wb-glass absolute z-10 grid size-9 -translate-x-1/2 -translate-y-1/2 place-items-center rounded-full"
              style={{ left: m.x, top: m.y }}
            >
              <ChevronRight className="size-4" style={{ transform: `rotate(${m.angle}deg)` }} aria-hidden />
            </button>
          )
        })}
      {!outline && clusterOpen && !composerOpen && (
        <Cluster
          store={store}
          onActivity={() => timer.activity()}
          onAdd={(k: AddKind) => void add(k)}
          onFit={() => handle.current?.fit(store.getState().selection.length ? 'selection' : 'all')}
          onOutline={() => {
            timer.hide()
            setOutline(true)
          }}
          onUndo={() => handle.current?.undo()}
          onRedo={() => handle.current?.redo()}
          canUndo={handle.current?.canUndo() ?? false}
          canRedo={handle.current?.canRedo() ?? false}
          more={more}
          addable={readOnly ? { sticky: false, note: false, web: false, image: false, clipboard: false, artifact: false, diff: false, terminal: false } : addable}
        />
      )}
      {!embedded && onClose && clusterOpen && !outline && (
        <button type="button" aria-label="Close the whiteboard" onClick={onClose} className="wb-glass wb-rise absolute top-[calc(16px+var(--safe-top,0px))] right-[calc(24px+var(--safe-right,0px))] z-20 grid size-11 place-items-center rounded-full">
          <X className="size-5" />
        </button>
      )}
      {bubble && (!anchor || outline) && bubbleNode}
      {composerOpen ? (
        <Sheet
          pick={pick}
          board={board}
          plan={plan}
          offScreen={offScreen}
          onOffScreen={(on) => {
            setOffScreen(on)
            replan(on)
          }}
          onRemove={(i) => setPlan((p) => withoutImage(p, i))}
          compose={composeThumb}
          noVision={noVision}
          sending={sending}
          text={text}
          onText={setText}
          onSend={() => void send()}
          onClose={closeComposer}
          onOpenSession={() => openSession()}
          autoFocus={focusField}
        />
      ) : (
        <Pill pick={pick} status={status} onGrabber={() => timer.grabber()} onOpen={() => openComposer()} mic={mic} recording={recordingRow} />
      )}
      {card && <CardSheet card={card} phone={phone} blobUrl={host.blobUrl} onClose={() => setOpenCard(null)} onSession={(s) => openSession(s)} />}
      {chooser && <ChooserDialog c={chooser} onDone={() => setChooser(null)} />}
      <input
        ref={fileInput}
        type="file"
        accept="image/*"
        className="hidden"
        onChange={(e) => {
          const f = e.target.files?.[0]
          e.target.value = ''
          if (f) void placeImage(provider.doc, spot, host.blobPut, f, f.name.replace(/\.[^.]+$/, '') || 'Image').then(select, (err: Error) => notify.error(err.message))
        }}
      />
    </div>
  )
}

function ToolbarButton({ onClick, children }: { onClick(): void; children: React.ReactNode }) {
  return (
    <button type="button" onClick={onClick} className="min-h-9 rounded-lg bg-layer-1 px-3 text-sm text-fg-primary hover:bg-layer-2 focus-visible:outline focus-visible:outline-2 focus-visible:outline-accent">
      {children}
    </button>
  )
}

function ChooserDialog({ c, onDone }: { c: Chooser; onDone(): void }) {
  const [value, setValue] = useState('input' in c ? c.input : '')
  return (
    <div className="absolute inset-0 z-50 grid place-items-center bg-black/20" onPointerDown={(e) => e.target === e.currentTarget && onDone()}>
      <div role="dialog" aria-label={c.title} className="wb-glass flex max-h-[70%] w-[min(360px,calc(100vw-32px))] flex-col gap-2 rounded-2xl p-3" onKeyDown={(e) => e.key === 'Escape' && onDone()}>
        <h2 className="px-1 text-sm font-semibold">{c.title}</h2>
        {'items' in c ? (
          c.items.length === 0 ? (
            <p className="wb-muted px-1 text-sm">Nothing to add from this session yet.</p>
          ) : (
            <ul className="flex flex-col overflow-y-auto">
              {c.items.map((i) => (
                <li key={i.key}>
                  <button type="button" onClick={() => (i.run(), onDone())} className="wb-press flex min-h-11 w-full items-center gap-2 rounded-xl px-3 text-left text-sm">
                    <span className="min-w-0 flex-1 truncate">{i.label}</span>
                    {i.detail && <span className="wb-muted text-xs">{i.detail}</span>}
                  </button>
                </li>
              ))}
            </ul>
          )
        ) : (
          <form
            className="flex gap-2"
            onSubmit={(e) => {
              e.preventDefault()
              c.run(value)
              onDone()
            }}
          >
            {/* biome-ignore lint/a11y/noAutofocus: the dialog exists to take this one value */}
            <input autoFocus value={value} onChange={(e) => setValue(e.target.value)} placeholder={c.placeholder} className="wb-chip min-w-0 flex-1 rounded-lg px-3 py-2 text-sm outline-none" />
            <button type="submit" className="rounded-lg bg-accent px-3 text-sm font-semibold text-white">
              Add
            </button>
          </form>
        )}
      </div>
    </div>
  )
}

const BGS: Meta['bg'][] = ['dots', 'grid', 'paper', 'none']

function MoreMenu(p: {
  title: string
  onRename(t: string): void
  meta: Meta
  onBg(bg: Meta['bg']): void
  store: ReturnType<typeof createWhiteboardStore>
  archived: { id: Id; title: string }[]
  onRestore(id: Id): void
  onCopy(): void
  onClose?: () => void
}) {
  const [name, setName] = useState(p.title)
  const v = useStore(p.store)
  const toggle = (label: string, on: boolean, patch: () => void) => (
    <button type="button" role="menuitemcheckbox" aria-checked={on} onClick={patch} className="wb-press flex min-h-11 w-full items-center gap-2 rounded-xl px-3 text-left text-sm">
      <span className="grid size-4 place-items-center">{on && <Check className="size-4" />}</span>
      {label}
    </button>
  )
  return (
    <div className="flex flex-col gap-1" role="menu" aria-label="Whiteboard options">
      <form
        className="px-2 pt-1"
        onSubmit={(e) => {
          e.preventDefault()
          if (name.trim()) p.onRename(name.trim())
        }}
      >
        <label className="wb-muted text-[11px] font-semibold uppercase tracking-wide" htmlFor="wb-title">
          Title
        </label>
        <input id="wb-title" value={name} onChange={(e) => setName(e.target.value)} onBlur={() => name.trim() && p.onRename(name.trim())} className="wb-chip mt-1 w-full rounded-lg px-2 py-1.5 text-sm outline-none" />
      </form>
      <div className="px-2">
        <div className="wb-muted pt-1 text-[11px] font-semibold uppercase tracking-wide">Background</div>
        <div className="mt-1 flex gap-1" role="radiogroup" aria-label="Background">
          {BGS.map((bg) => (
            <button key={bg} type="button" role="radio" aria-checked={p.meta.bg === bg} onClick={() => p.onBg(bg)} className="wb-press rounded-lg px-2 py-1.5 text-xs capitalize" data-on={p.meta.bg === bg}>
              {bg}
            </button>
          ))}
        </div>
      </div>
      <div className="my-1 h-px bg-current opacity-10" />
      {toggle('Show answered marks', v.showAnswered, () => v.set({ showAnswered: !v.showAnswered }))}
      {toggle('Keep controls open', v.keepControls, () => v.set({ keepControls: !v.keepControls }))}
      {toggle('Draw with finger', v.fingerInk, () => v.set({ fingerInk: !v.fingerInk }))}
      {toggle('Native Pencil ink', v.inkEngine === 'auto', () => v.set({ inkEngine: v.inkEngine === 'auto' ? 'pointer' : 'auto' }))}
      <button type="button" onClick={p.onCopy} className="wb-press flex min-h-11 w-full items-center rounded-xl px-3 pl-9 text-left text-sm">
        Copy outline
      </button>
      {p.archived.length > 0 && (
        <>
          <div className="my-1 h-px bg-current opacity-10" />
          <div className="wb-muted px-3 text-[11px] font-semibold uppercase tracking-wide">Archived cards</div>
          {p.archived.map((a) => (
            <button key={a.id} type="button" onClick={() => p.onRestore(a.id)} className="wb-press flex min-h-11 w-full items-center gap-2 rounded-xl px-3 text-left text-sm">
              <span className="min-w-0 flex-1 truncate">{a.title || 'Untitled'}</span>
              <span className="wb-muted text-xs">Restore</span>
            </button>
          ))}
        </>
      )}
      <div className="my-1 h-px bg-current opacity-10" />
      <a href="https://reactflow.dev" target="_blank" rel="noreferrer" className="wb-muted px-3 py-2 text-xs">
        Canvas by React Flow
      </a>
      {p.onClose && (
        <button type="button" onClick={p.onClose} className="wb-press flex min-h-11 w-full items-center rounded-xl px-3 pl-9 text-left text-sm">
          Close the whiteboard
        </button>
      )}
    </div>
  )
}
