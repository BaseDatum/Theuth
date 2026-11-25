// §8.5 renderers by kind (live LOD). Each fills the card's body; the chrome (title bar, shield, ink, scrubber) is
// CardNode's. Heavy live views (pages) are budgeted by lod.ts; terminals and web views never run live on the canvas.
import { AppWindow, FileCode2, FileDiff, FileText, Globe, Image as ImageIcon, MessageSquare, Shapes, StickyNote, Terminal as TerminalIcon, type LucideIcon } from 'lucide-react'
import { Component, lazy, Suspense, useEffect, useState, type ErrorInfo, type ReactNode } from 'react'
import { useQuery } from '@tanstack/react-query'
import type { CustomRendererProps } from 'streamdown'
import { useFileUrl } from '@/components/chat/Attachments'
import { privateOwner } from '@/lib/private'
import { useApi } from '@/lib/rpc'
import type { DiffSpec } from '@/rpc/bindings/method.artifact.input'
import { useQuerySupport } from '@/rpc/capabilities'
import { useSessions } from '@/store/sessions'
import { useCanvas } from '../context'
import type { Card, CardKind, Source } from '../model'
import { STICKY_MAX, setSticky } from '../ops'
import { EXCERPT_PAD, EXCERPT_ROW } from '../send'
import { lastTurns } from '../turns'
import { PageCard, useArtifact } from './PageCard'

const MermaidFence = lazy(() => import('@/components/chat/Mermaid').then((m) => ({ default: m.MermaidFence })))
const JsonRenderFence = lazy(() => import('@/components/chat/surface/JsonRenderFence').then((m) => ({ default: m.JsonRenderFence })))
const OpenUIFence = lazy(() => import('@/components/chat/surface/OpenUIFence').then((m) => ({ default: m.OpenUIFence })))
const DiffBody = lazy(() => import('@/components/code/DiffBody').then((m) => ({ default: m.DiffBody })))
const NoteView = lazy(() => import('@/components/notes/NotePage').then((m) => ({ default: m.NoteView })))

export const KIND_ICON: Record<CardKind, LucideIcon> = {
  page: AppWindow, diagram: Shapes, note: FileText, excerpt: FileCode2, diff: FileDiff, terminal: TerminalIcon,
  web: Globe, image: ImageIcon, sticky: StickyNote, session: MessageSquare,
}
export const KIND_NAME: Record<CardKind, string> = {
  page: 'page', diagram: 'diagram', note: 'note', excerpt: 'excerpt', diff: 'diff', terminal: 'terminal',
  web: 'web', image: 'image', sticky: 'sticky', session: 'session',
}

const fence = (code: string, language: string) => ({ code, isIncomplete: false, language }) as unknown as CustomRendererProps
const Pending = () => <div className="absolute inset-0 animate-pulse bg-layer-2/60" />

function Info({ icon: Icon, lines, children }: { icon: LucideIcon; lines: string[]; children?: ReactNode }) {
  return (
    <div className="wb-chip">
      <Icon className="size-6 opacity-70" aria-hidden />
      {lines.map((l, i) => (
        <span key={i} className={i === 0 ? 'max-w-full truncate text-xs font-medium' : 'max-w-full truncate text-[11px] text-fg-tertiary'}>
          {l}
        </span>
      ))}
      {children}
    </div>
  )
}

function Sticky({ card, text, editing }: { card: Card; text: string; editing: boolean }) {
  const { doc, readOnly, enter, stopCapturing } = useCanvas()
  const [draft, setDraft] = useState(text)
  useEffect(() => setDraft(text), [text])
  if (editing && !readOnly) {
    return (
      <div className="wb-sticky-text nodrag nopan nowheel">
        <textarea
          aria-label={`${card.title || 'Sticky'} text`}
          autoFocus
          maxLength={STICKY_MAX}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          onBlur={() => {
            if (draft !== text) {
              setSticky(doc, card.id, draft)
              stopCapturing()
            }
            enter(null)
          }}
          onKeyDown={(e) => {
            if (e.key === 'Escape') (e.target as HTMLTextAreaElement).blur()
            e.stopPropagation()
          }}
        />
      </div>
    )
  }
  return <div className="wb-sticky-text">{text}</div>
}

function Excerpt({ s }: { s: Extract<Source, { t: 'excerpt' }> }) {
  const lines = s.text.split('\n')
  const width = String(s.start + lines.length).length
  return (
    <pre className="absolute inset-0 m-0 overflow-hidden font-mono text-[12px] text-fg-primary" style={{ padding: `${EXCERPT_PAD}px 0`, lineHeight: `${EXCERPT_ROW}px` }}>
      {lines.map((l, i) => (
        <div key={i} className="flex whitespace-pre" style={{ height: EXCERPT_ROW }}>
          <span className="select-none pr-3 pl-3 text-right text-fg-tertiary" style={{ minWidth: `${width + 3}ch` }}>
            {s.start + i}
          </span>
          <span>{l}</span>
        </div>
      ))}
    </pre>
  )
}

function Picture({ src, title }: { src: string | null; title: string }) {
  if (src === null) return <Pending />
  if (src === '') return <Info icon={ImageIcon} lines={[title || 'Image', 'image unavailable']} />
  return <img src={src} alt={title} className="absolute inset-0 size-full object-contain" draggable={false} />
}

/** An engine file (`path`), read through the shared file cache (`asset://` locally, else bytes over RPC). */
function PathImage({ path, session, title }: { path: string; session?: string; title: string }) {
  return <Picture src={useFileUrl(path, session)} title={title} />
}

/** An image artifact of a session: its path from the artifact list, then as a file. */
function ArtifactImage({ session, callId, title }: { session: string; callId: string; title: string }) {
  const { artifact, gone } = useArtifact(session, callId)
  if (artifact) return <PathImage path={artifact.path} session={privateOwner(artifact.session)} title={title} />
  return gone ? <Info icon={ImageIcon} lines={[title || 'Image', 'source gone']} /> : <Pending />
}

function BlobImage({ blob, title }: { blob: string; title: string }) {
  const { host } = useCanvas()
  const [url, setUrl] = useState<string | null>(null)
  useEffect(() => {
    let alive = true
    setUrl(null)
    void host.blobUrl(blob).then((u) => alive && setUrl(u ?? ''))
    return () => {
      alive = false
    }
  }, [blob, host])
  return <Picture src={url} title={title} />
}

function ImageBody({ s, title }: { s: Extract<Source, { t: 'image' }>; title: string }) {
  if (s.blob) return <BlobImage blob={s.blob} title={title} />
  if (s.path) return <PathImage path={s.path} session={s.session ? privateOwner(s.session) : undefined} title={title} />
  if (s.call_id && s.session) return <ArtifactImage session={s.session} callId={s.call_id} title={title} />
  return <Info icon={ImageIcon} lines={[title || 'Image', 'no source']} />
}

const TAIL = 40

/** The session's last turns, read-only: live from the session store when that session is loaded, else one history page. */
function SessionBody({ id, title }: { id: string; title: string }) {
  const api = useApi()
  const s = useSessions((st) => st.sessions[id])
  const can = useQuerySupport(['load_session'])
  const loaded = s !== undefined && s.entries.length > 0
  const page = useQuery({
    enabled: can && !loaded,
    queryKey: ['whiteboard', 'session-tail', id],
    queryFn: () => api.load_session(id, null, TAIL),
    staleTime: 30_000,
  })
  const turns = lastTurns(loaded ? s.entries : (page.data?.history ?? []))
  const name = s?.title || page.data?.name || title || 'Session'
  if (!loaded && page.isPending && can) return <Pending />
  if (turns.length === 0) return <Info icon={MessageSquare} lines={[name, page.isError || !can ? 'session unavailable' : 'no turns yet']} />
  return (
    <div className="absolute inset-0 flex flex-col justify-end gap-1.5 overflow-hidden p-2.5 text-[12px] leading-snug">
      {turns.map((t, i) => (
        <div
          key={i}
          className={
            t.role === 'user'
              ? 'max-w-[85%] self-end rounded-xl rounded-br-sm bg-layer-2 px-2.5 py-1.5 text-fg-primary'
              : 'max-w-[95%] self-start px-0.5 text-fg-secondary'
          }
        >
          <p className="line-clamp-4 whitespace-pre-wrap break-words">{t.text}</p>
        </div>
      ))}
    </div>
  )
}

/** One card's renderer failing must not take the whole whiteboard down: it shows a chip saying so. */
export class CardBoundary extends Component<{ card: Card; children: ReactNode }, { failed: boolean }> {
  state = { failed: false }
  static getDerivedStateFromError() {
    return { failed: true }
  }
  componentDidCatch(error: unknown, info: ErrorInfo) {
    console.error('whiteboard card render failed', this.props.card.id, error, info.componentStack)
  }
  componentDidUpdate(prev: { card: Card }) {
    // A new version (or a retitle) gets a fresh try.
    if (this.state.failed && prev.card.versions !== this.props.card.versions) this.setState({ failed: false })
  }
  render() {
    if (!this.state.failed) return this.props.children
    const Icon = KIND_ICON[this.props.card.kind]
    return <Info icon={Icon} lines={[this.props.card.title || KIND_NAME[this.props.card.kind], 'could not render here — open it']} />
  }
}

/** The live body of a card, by its version's source. */
export function LiveBody({ card, source, editing }: { card: Card; source: Source | undefined; editing: boolean }) {
  if (!source) return <Info icon={KIND_ICON[card.kind]} lines={[card.title || KIND_NAME[card.kind], 'no content']} />
  switch (source.t) {
    case 'sticky':
      return <Sticky card={card} text={source.text} editing={editing} />
    case 'page':
      return <PageCard card={card} callId={source.call_id} session={source.session} live />
    case 'diagram':
      return (
        <div className="absolute inset-0 overflow-hidden p-2 [&_svg]:max-h-full [&_svg]:max-w-full">
          <Suspense fallback={<Pending />}>
            {source.lang === 'mermaid' ? (
              <MermaidFence {...fence(source.code, 'mermaid')} fill />
            ) : source.lang === 'openui' ? (
              <OpenUIFence {...fence(source.code, 'openui')} />
            ) : (
              <JsonRenderFence {...fence(source.code, 'json-render')} />
            )}
          </Suspense>
        </div>
      )
    case 'excerpt':
      return <Excerpt s={source} />
    case 'diff':
      return (
        <div className="absolute inset-0 overflow-hidden">
          <Suspense fallback={<Pending />}>
            {/* The whiteboard schema stores the 212 diff spec as opaque JSON (§2.2); the engine validated it on write. */}
            <DiffBody session={source.frozen?.session ?? source.session} spec={source.spec as DiffSpec} />
          </Suspense>
          {source.as_of !== undefined && (
            <span className="absolute right-2 bottom-2 rounded bg-layer-1 px-1.5 text-[10px] text-fg-tertiary">
              as of {new Date(source.as_of).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })}
            </span>
          )}
        </div>
      )
    case 'note':
      return (
        <div className="absolute inset-0 overflow-hidden">
          <Suspense fallback={<Pending />}>
            <NoteView id={source.doc} embedded hidePopout />
          </Suspense>
        </div>
      )
    case 'terminal':
      return <Info icon={TerminalIcon} lines={[card.title || 'Terminal', `job ${source.job}`, 'open to watch it live']} />
    case 'web':
      return <Info icon={Globe} lines={[card.title || 'Web', hostOf(source.url)]} />
    case 'image':
      return <ImageBody s={source} title={card.title} />
    case 'session':
      return <SessionBody id={source.session} title={card.title} />
  }
}

export function hostOf(url: string): string {
  try {
    return new URL(url).host
  } catch {
    return url
  }
}

/** Kind icon + title: below 60 px on screen, or when nothing better exists yet. */
export function Chip({ card }: { card: Card }) {
  const Icon = KIND_ICON[card.kind]
  return (
    <div className="wb-chip" aria-hidden>
      <Icon className="size-[28%] max-h-16 min-h-4 opacity-60" />
      <span className="max-w-full truncate text-[max(11px,8%)] font-semibold">{card.title || KIND_NAME[card.kind]}</span>
    </div>
  )
}

/** A stored snapshot `snap/<card>/<v>`. */
export function Snapshot({ card }: { card: Card }) {
  const { host } = useCanvas()
  const [url, setUrl] = useState<string | null>(null)
  const key = card.snap ? `snap/${card.id}/${card.snap.v}` : null
  useEffect(() => {
    let alive = true
    setUrl(null)
    if (key) void host.blobUrl(key).then((u) => alive && setUrl(u))
    return () => {
      alive = false
    }
  }, [key, host])
  if (!url) return <Chip card={card} />
  return <img src={url} alt="" className="absolute inset-0 size-full object-cover object-top" draggable={false} />
}
