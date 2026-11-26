// 218 §8.8: the whiteboard — full window from the route (over sidebar and status bar), or inside a tile. Owns the view
// store, the CanvasHost, the lazy canvas, and the shell around it: the composer dock (the session view's composer),
// cluster, reply bubble, card sheet, outline view, the Pencil gestures and the keyboard. Reads the document through
// `useBoard`, writes through `canvas/ops` and the canvas handle; RPCs through `wbRpc`.

import './glass.css'
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { Check, ChevronRight, X } from 'lucide-react'
import { Suspense, lazy, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { useStore } from 'zustand'
import { useNoteTitle } from '@/components/notes/NotePage'
import { useDocument } from '@/components/editor/useDocument'
import { useIsMobile } from '@/hooks/use-mobile'
import { PLAIN_OPTIONS, useDrafts } from '@/lib/drafts'
import { putAttachment } from '@/lib/files'
import { usePointerCoarse } from '@/lib/mobile'
import { notify } from '@/lib/notify'
import { base64Blob, wbCapture } from '@/lib/wb-capture'
import { queryKeys, useApi } from '@/lib/rpc'
import { listenPencil, recognizeText } from '@/lib/ink-native'
import type { EngineProvider } from '@/lib/doc/provider'
import { useQuerySupport } from '@/rpc/capabilities'
import { useConnection } from '@/store/connection'
import { useApprovals } from '@/store/approvals'
import { useNotices } from '@/store/notices'
import { statusLine, useSessionField, useSessions } from '@/store/sessions'
import { ApprovalCard } from '@/components/chat/ApprovalCard'
import { useTerminals } from '@/components/chat/TerminalViewer'
import { type AddKind, Cluster, type ClusterProps } from './Cluster'
import { Bubble } from './Bubble'
import { CardSheet } from './CardSheet'
import { OutlineView, outlineText } from './OutlineView'
import { type SessionPick, WhiteboardComposer, useSessionTitle } from './WhiteboardComposer'
import { asUrl, excerptOf, parseFileRef, place, placeFromClipboard, placeImage } from './add'
import { ViewPicker } from './ViewPicker'
import { bubbleAnchor, lastReply } from './reply'
import type { PlannedImage, SendPlan, WhiteboardCanvasHandle } from './canvas/api'
import type { Board, Id, Meta } from './canvas/model'
import * as ops from './canvas/ops'
import { createWhiteboardStore } from './canvas/store'
import { useBoard } from './canvas/useBoard'
import { clusterTimer } from './cluster-timer'
import { createHost } from './host'
import { briefMarkdown } from './export'
import { type EdgeMark, FOLLOW_PAUSE_MS, edgeMarks } from './edges'
import { pencilGestures } from './pencil-gestures'
import { imageKey, sendWhiteboard, withoutImage } from './send/send'
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
  return <WhiteboardShell key={props.doc} {...props} provider={live.provider} frame={frame} />
}

type Chooser = { title: string; items: { key: string; label: string; detail?: string; run(): void }[] } | { title: string; input: string; placeholder: string; run(value: string): void }

const cardsOf = (b: Board) => Object.values(b.cards).filter((c) => c.state !== 'archived').length
const NO_PLAN: SendPlan = { marks: [], images: [], offScreen: 0 }

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

/** The whiteboard on an open document (tests and the dev fixture mount it on a local provider). */
export function WhiteboardShell({ doc, session: wanted = null, embedded = false, onSession, onClose, provider, frame }: WhiteboardViewProps & { provider: EngineProvider; frame: string }) {
  const api = useApi()
  const client = useConnection((s) => s.client)
  const qc = useQueryClient()
  const navigate = useNavigate()
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
  const [pickingView, setPickingView] = useState(false)
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
    onOpen: (s) => void navigate({ to: '/c/$session', params: { session: s } }),
  }

  // ── The composer dock: its top edge (from the frame's bottom, plus a gap) is `--wb-dock`, what floats above it.
  const frameEl = useRef<HTMLDivElement>(null)
  const dock = useRef<HTMLDivElement>(null)
  useLayoutEffect(() => {
    const f = frameEl.current
    const d = dock.current
    if (!f || !d) return
    const measure = () => f.style.setProperty('--wb-dock', `${Math.round(f.getBoundingClientRect().bottom - d.getBoundingClientRect().top + 12)}px`)
    const ro = new ResizeObserver(measure)
    ro.observe(f)
    ro.observe(d)
    measure()
    return () => ro.disconnect()
  }, [])

  // ── Notices while full-window show above the composer (the status bar is covered).
  const notice = useNotices((s) => s.current)
  const [noticeShown, setNoticeShown] = useState<string | null>(null)
  useEffect(() => {
    if (embedded || !notice || notice.dismissed) return setNoticeShown(null)
    setNoticeShown(notice.text)
    const t = setTimeout(() => setNoticeShown(null), Math.max(0, notice.expiresAt - Date.now()))
    return () => clearTimeout(t)
  }, [notice, embedded])
  const working = running ? (statusLine(activity ?? 'thinking', statusText) ?? (activity === 'tool' ? 'working…' : activity === 'idle' || !activity ? 'thinking…' : `${activity}…`)) : null
  // A tool call of the target session waiting on Allow / Deny (a wb_* write in the default approval mode): full-window,
  // the transcript is covered, so the card shows here and the pill says so instead of "working…".
  const waiting = useApprovals((s) => s.pending.find((p) => p.session === session))
  const status = noticeShown ?? (waiting ? `waiting for you: ${waiting.tool}` : working)

  // ── Cluster: timer (B4) mirrored into the store (B3 reads it for the attribution panel).
  const timer = useMemo(() => clusterTimer({ onChange: (open) => store.getState().set({ clusterOpen: open }) }), [store])
  useEffect(() => () => timer.dispose(), [timer])
  const clusterOpen = useStore(store, (s) => s.clusterOpen)
  const keepControls = useStore(store, (s) => s.keepControls)
  useEffect(() => timer.keep(keepControls), [timer, keepControls])
  useEffect(() => store.subscribe((s, p) => s.penDown && !p.penDown && timer.pen()), [store, timer])

  // ── Pencil double-tap / squeeze (iPad): the plugin forwards the gesture; the store stays the only writer of the tool.
  useEffect(() => {
    if (readOnly) return
    const gestures = pencilGestures(store, () => timer.show())
    let gone = false
    let unlisten: (() => void) | null = null
    void listenPencil(gestures.on).then((u) => {
      if (gone) u()
      else unlisten = u
    })
    return () => {
      gone = true
      unlisten?.()
      gestures.dispose()
    }
  }, [store, timer, readOnly])

  // ── Composer tray: what a send carries besides the words, re-planned as the board and the view change; an image
  // removed from the tray stays removed (by its key) and the off-screen opt-in stays on until the next send.
  const [planned, setPlanned] = useState<SendPlan>(NO_PLAN)
  const [offScreen, setOffScreen] = useState(false)
  const [removed, setRemoved] = useState<ReadonlySet<string>>(() => new Set())
  const replanTimer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const replan = useCallback(() => {
    clearTimeout(replanTimer.current)
    replanTimer.current = setTimeout(() => setPlanned(handle.current?.planSend({ offScreen }) ?? NO_PLAN), 200)
  }, [offScreen])
  useEffect(() => {
    replan()
    return () => clearTimeout(replanTimer.current)
  }, [replan, board])
  const plan = useMemo(() => [...removed].reduce((p, key) => withoutImage(p, p.images.findIndex((i) => imageKey(i) === key)), planned), [planned, removed])
  const composeThumb = useCallback((i: PlannedImage) => handle.current?.composeImage(i, 160) ?? Promise.reject(new Error('no canvas')), [])
  const can = useQuerySupport(['list_models', 'vision_model'])
  const models = useQuery({ enabled: can, queryKey: queryKeys.models, queryFn: () => api.list_models() }).data
  const visionModel = useQuery({ enabled: can, queryKey: queryKeys.visionModel, queryFn: () => api.vision_model() }).data
  // Unbound (R1): the new session's pick from the composer's picker (the new-chat draft's, A2), else the engine default.
  const draftModel = useDrafts((s) => s.pick.model)
  const sessionModel = useSessionField(session ?? '', 'model')
  const model = (session ? sessionModel : draftModel) ?? useConnection.getState().info?.default_model
  const info = models?.find((m) => m.id === model)
  const noVision = !!info && !info.image && !visionModel

  // ── The reply bubble: after a turn of the target session ends.
  type BubbleState = { text: string; turn: WbTurnInfo | null; session: string }
  const [bubble, setBubble] = useState<BubbleState | null>(null)
  const [undoing, setUndoing] = useState(false)
  // The whiteboard does not stream the session's transcript (only its state), so the turn's final text is not in the
  // store when `running` drops: the end of a turn re-reads the history and arms `awaiting`; the bubble opens once that
  // session has a reply row.
  const [awaiting, setAwaiting] = useState<string | null>(null)
  const wasRunning = useRef(running)
  useEffect(() => {
    const was = wasRunning.current
    wasRunning.current = running
    if (running) return setAwaiting(null)
    if (!session || !was) return
    setAwaiting(session)
    api.load_session(session, null, 60).then(
      (page) => useSessions.getState().loadHistory(session, page, 'replace'),
      () => {},
    )
  }, [running, session, api])
  // Subscribed only while awaiting: streaming deltas of a running turn do not re-render the view.
  const reply = useSessions((s) => (awaiting ? lastReply(s.sessions[awaiting]?.entries ?? []) : null))
  useEffect(() => {
    if (!awaiting || !reply) return
    const s = awaiting
    setAwaiting(null)
    setBubble({ text: reply, turn: null, session: s })
    rpc?.turn(doc, s).then(
      (turn) => setBubble((b) => (b && b.session === s ? { ...b, turn } : b)),
      () => {},
    )
  }, [awaiting, reply, rpc, doc])
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

  /** The composer's plain send (typed, dictated, or a voice note): the words and its attachments go with the
   * whiteboard — the tray's images and marks, handwriting, the selection, the context — into the picked session, or
   * a new one made by this first send (R1). Answers the session it went to; a failure leaves the ink fresh (M2). */
  const deliver = async (m: { text: string; files: string[]; clientId: string }): Promise<string> => {
    const h = handle.current
    if (!h || !rpc) throw new Error('The whiteboard is still opening')
    setBubble(null)
    try {
      const r = await sendWhiteboard(
        {
          handle: h,
          capture: host.capture,
          recognize: recognizeText,
          wb: rpc,
          putAttachment: (blob, ext, s) => putAttachment(api, blob, ext, s),
          sendInput: (s, body, files, id) => api.send_input(s, body, files, null, id),
          steer: (s, body, id, files) => api.steer(s, body, id, files),
          createSession: async (p) => (await api.create_session({ ...PLAIN_OPTIONS, model: p.model, effort: p.effort })).session,
          dpr: globalThis.devicePixelRatio || 1,
        },
        {
          doc,
          title: title || 'Whiteboard',
          cards: cardsOf(board),
          text: m.text,
          session,
          running,
          bound: !!session && boundList.some((b) => b.session === session),
          vision: !noVision,
          selection: store.getState().selection,
          clientId: m.clientId,
          // The tray as shown when it was edited; else planned now (the marks in view at the moment of sending).
          plan: removed.size > 0 || offScreen ? plan : undefined,
          frameOf: (c) => board.cards[c]?.frame,
          pick: session ? undefined : useDrafts.getState().pick,
          files: m.files,
        },
      )
      return r.session
    } catch (e) {
      throw new Error(`Not sent — your marks are kept for the next try: ${e instanceof Error ? e.message : String(e)}`)
    }
  }
  const sent = (s: string) => {
    // A first send (R1) made the session: it becomes the target (and `?session=`); the engine's echo fills its rows.
    if (s && s !== session) {
      setTarget(s)
      useSessions.getState().ensure(s)
      // The pick went into this session (as `openDraftSession` spends it).
      useDrafts.getState().resetPick()
    }
    refreshBound()
    setRemoved(new Set())
    setOffScreen(false)
    replan()
  }

  // ── Add.
  const terminals = useTerminals(session ?? '')
  const spot = (w: number, h: number) => handle.current?.freeSpot(w, h) ?? { x: 0, y: 0 }
  const select = (id: Id | null) => id && store.getState().set({ selection: [id] })
  const add = async (kind: AddKind) => {
    const d = provider.doc
    try {
      switch (kind) {
        case 'sticky': {
          // An empty sticky opens for typing at once (entered = its textarea), as a new note would.
          const id = place(d, spot, 'sticky', 'Sticky', { t: 'sticky', text: '' })
          return store.getState().set({ selection: [id], entered: id })
        }
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
        case 'view':
          return setPickingView(true)
        case 'clipboard': {
          const id = await placeFromClipboard(d, spot, host.blobPut)
          return id ? select(id) : notify('The clipboard is empty')
        }
        case 'excerpt':
          if (!session) return
          return setChooser({
            title: 'Add a file excerpt',
            input: '',
            placeholder: 'src/app.ts:10-40',
            run: (v) => {
              const ref = parseFileRef(v)
              if (!ref) return
              api.file_view_read(session, ref.path, null).then(
                (f) => {
                  const source = excerptOf(ref.path, f.text, f.lang || undefined, ref.start, ref.end)
                  const name = ref.path.split('/').pop() || ref.path
                  select(place(d, spot, 'excerpt', `${name} ${source.start}–${source.end}`, source))
                },
                (e: Error) => notify.error(`Could not read ${ref.path}: ${e.message}`),
              )
            },
          })
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
  const addable: Record<AddKind, boolean> = { sticky: true, note: true, web: true, image: true, view: true, clipboard: true, artifact: !!session, excerpt: !!session, diff: !!session, terminal: !!session && terminals.length > 0 }

  // ── Keyboard (desktop, full window): V H P E L F N O, ⌘0, ⌘↵, Esc.
  useEffect(() => {
    if (embedded) return
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement | null
      if (t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName))) return
      const st = store.getState()
      const mod = e.metaKey || e.ctrlKey
      if (mod && e.key === '0') return e.preventDefault(), handle.current?.fit('all')
      if (mod && e.key === 'Enter') return e.preventDefault(), dock.current?.querySelector('textarea')?.focus()
      if (mod || e.altKey) return
      const k = e.key.toLowerCase()
      if (e.key === 'Escape') {
        if (openCard) return setOpenCard(null)
        if (st.entered) return st.set({ entered: null })
        return timer.hide()
      }
      const modes: Record<string, 'select' | 'hand' | 'draw' | 'erase' | 'link' | 'frame'> = { v: 'select', h: 'hand', p: 'draw', e: 'erase', l: 'link', f: 'frame' }
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
    // Not over the composer, which sits in that quadrant.
    const over = e.target instanceof Element && e.target.closest('[data-whiteboard-composer]') !== null
    const idle = () => !store.getState().clusterOpen
    rest.current = inQuad && !over && idle() ? setTimeout(() => idle() && timer.show(), 600) : null
    if (store.getState().clusterOpen) timer.activity()
  }

  // ── Export (P5, cheap parts): a note brief, and the current view as a PNG on the clipboard.
  const exportBrief = async () => {
    try {
      const n = await api.doc_create(`${title || 'Whiteboard'} — brief`, briefMarkdown(board, title, doc), null)
      notify.success('Brief saved as a note', { action: { label: 'Open', onClick: () => void navigate({ to: '/notes/$id', params: { id: n.id } }) } })
    } catch (e) {
      notify.error(e instanceof Error ? e.message : String(e))
    }
  }
  /** The ClipboardItem is built inside the click (Safari/WKWebView keep the gesture for a promised blob). */
  const copyViewPng = () => {
    const h = handle.current
    if (!h) return
    timer.hide()
    const png = (async () => {
      // Two frames: the cluster and its menu are gone from the capture.
      for (let i = 0; i < 2; i++) await new Promise<void>((r) => requestAnimationFrame(() => r()))
      const r = h.frameRect()
      const shot = await wbCapture({ x: r.left, y: r.top, width: r.width, height: r.height })
      if (!shot) throw new Error('Copying the view as a picture needs the desktop or iPad app')
      return base64Blob(shot.png)
    })()
    const write = typeof ClipboardItem === 'function' && navigator.clipboard?.write ? navigator.clipboard.write([new ClipboardItem({ 'image/png': png })]) : Promise.reject(new Error('This window cannot copy pictures'))
    write.then(
      () => notify.success('View copied as a picture'),
      (e: unknown) => notify.error(e instanceof Error ? e.message : String(e)),
    )
    png.catch(() => {})
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
      onBrief={() => void exportBrief()}
      onPng={copyViewPng}
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
  const controls: Omit<ClusterProps, 'onHold'> = {
    store,
    onActivity: () => timer.activity(),
    onAdd: (k: AddKind) => void add(k),
    onFit: () => handle.current?.fit(store.getState().selection.length ? 'selection' : 'all'),
    onOutline: () => {
      timer.hide()
      setOutline(true)
    },
    onUndo: () => handle.current?.undo(),
    onRedo: () => handle.current?.redo(),
    canUndo: handle.current?.canUndo() ?? false,
    canRedo: handle.current?.canRedo() ?? false,
    more,
    addable: readOnly ? { sticky: false, note: false, web: false, image: false, view: false, clipboard: false, artifact: false, excerpt: false, diff: false, terminal: false } : addable,
  }

  return (
    <div ref={frameEl} className={frame} data-whiteboard={doc} onPointerMove={onPointerMove}>
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
                  {!readOnly && <ToolbarButton onClick={() => void add('view')}>Add app view</ToolbarButton>}
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
      {/* The canvas stays mounted under the outline (its handle feeds the composer). */}
      {/* opacity-0 as well: React Flow sets `visibility: visible` inline on measured nodes, so `invisible` alone leaves cards over the outline. */}
      <div className={outline ? 'pointer-events-none invisible absolute inset-0 opacity-0' : 'absolute inset-0'} aria-hidden={outline} onPointerDownCapture={pauseFollow}>
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
            onCanvasTap={() => timer.tap()}
            onPenActivity={(down) => down && timer.pen()}
            onViewChange={() => {
              recomputeEdges()
              replan()
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
      {!outline && clusterOpen && <Cluster {...controls} onHold={timer.hold} />}
      {!embedded && onClose && clusterOpen && !outline && (
        <button type="button" aria-label="Close the whiteboard" onClick={onClose} className="wb-glass wb-rise absolute top-[calc(16px+var(--safe-top,0px))] right-[calc(24px+var(--safe-right,0px))] z-20 grid size-11 place-items-center rounded-full">
          <X className="size-5" />
        </button>
      )}
      {bubble && (!anchor || outline) && bubbleNode}
      {waiting && (
        <div
          className={`wb-rise absolute z-20 bottom-[var(--wb-dock,calc(80px+var(--safe-bottom,0px)))] w-[min(360px,calc(100vw-48px))] rounded-xl shadow-lg [&>*]:my-0 ${clusterOpen && !outline ? 'right-[calc(94px+var(--safe-right,0px))]' : 'right-[calc(24px+var(--safe-right,0px))]'}`}
          onPointerDown={(e) => e.stopPropagation()}
        >
          <ApprovalCard approval={waiting} />
        </div>
      )}
      <WhiteboardComposer
        doc={doc}
        pick={pick}
        tools={controls}
        tray={{
          board,
          plan,
          offScreen,
          onOffScreen: setOffScreen,
          onRemove: (i) => {
            const image = plan.images[i]
            if (image) setRemoved((r) => new Set(r).add(imageKey(image)))
          },
          compose: composeThumb,
          noVision,
          status,
        }}
        deliver={deliver}
        onSent={sent}
        dockRef={dock}
      />
      {card && <CardSheet card={card} phone={phone} blobUrl={host.blobUrl} onClose={() => setOpenCard(null)} onSession={(s) => openSession(s)} />}
      {chooser && <ChooserDialog c={chooser} onDone={() => setChooser(null)} />}
      {pickingView && <ViewPicker onPick={(path, t) => select(place(provider.doc, spot, 'view', t, { t: 'view', path }))} onDone={() => setPickingView(false)} />}
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
    <button type="button" onClick={onClick} className="min-h-9 rounded-lg bg-layer-1 px-3 text-sm text-fg-primary hover:bg-layer-2 focus-visible:outline focus-visible:outline-2 focus-visible:outline-fg-accent">
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
            <input autoFocus value={value} onChange={(e) => setValue(e.target.value)} placeholder={c.placeholder} className="wb-well min-w-0 flex-1 rounded-lg px-3 py-2 text-sm outline-none" />
            <button type="submit" className="rounded-lg bg-fg-accent px-3 text-sm font-semibold text-primary-foreground hover:bg-fg-accent-hover">
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
  onBrief(): void
  onPng(): void
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
        <input id="wb-title" value={name} onChange={(e) => setName(e.target.value)} onBlur={() => name.trim() && p.onRename(name.trim())} className="wb-well mt-1 w-full rounded-lg px-2 py-1.5 text-sm outline-none" />
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
      <button type="button" onClick={p.onCopy} className="wb-press flex min-h-11 w-full items-center rounded-xl px-3 pl-9 text-left text-sm">
        Copy outline
      </button>
      <button type="button" onClick={p.onBrief} className="wb-press flex min-h-11 w-full items-center rounded-xl px-3 pl-9 text-left text-sm">
        Export as a note
      </button>
      <button type="button" onClick={p.onPng} className="wb-press flex min-h-11 w-full items-center rounded-xl px-3 pl-9 text-left text-sm">
        Copy view as a picture
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
