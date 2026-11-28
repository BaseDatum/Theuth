// 218 §8.8 "Sheet drill-in" / design §4.4: a card's full view (note editor, live page, terminal, diff, …) sliding in
// from the right over 60 % of the whiteboard (the whole screen on a phone). The canvas stays where it was.

import { useQuery } from '@tanstack/react-query'
import { MessageSquare, X } from 'lucide-react'
import { lazy, Suspense, useEffect, useState } from 'react'
import type { CustomRendererProps } from 'streamdown'
import { useText } from '@/components/chat/ArtifactCard'
import { HtmlPage } from '@/components/chat/HtmlPage'
import { MermaidFence } from '@/components/chat/Mermaid'
import { Excerpt } from '@/components/chat/surface/Excerpt'
import { JsonRenderFence } from '@/components/chat/surface/JsonRenderFence'
import { OpenUIFence } from '@/components/chat/surface/OpenUIFence'
import { Terminal } from '@/components/chat/TerminalViewer'
import { DiffBody } from '@/components/code/DiffBody'
import { NoteView } from '@/components/notes/NotePage'
import { useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import type { DiffSpec } from '@/rpc/bindings/method.artifact.input'
import { useSessions } from '@/store/sessions'
import type { Card, Source } from './canvas/model'
import { versionOf } from './canvas/model'
import { KIND_LABEL } from './OutlineView'
import { WebSheetBody } from './canvas/cards/WebCard'
import { taskPath } from './task-cards'

// Lazy: it pulls the route tree, which holds the whiteboard route itself.
const ViewCard = lazy(() => import('./canvas/cards/ViewCard'))
const DeckSheetView = lazy(() => import('@/components/decks/DeckView').then((m) => ({ default: m.DeckView })))

function PageBody({ session, callId, title }: { session: string; callId: string; title: string }) {
  const api = useApi()
  const can = useQuerySupport(['list_artifacts'])
  const cached = useSessions((s) => s.sessions[session]?.artifacts.get(callId))
  const listed = useQuery({
    enabled: can && !cached,
    queryKey: ['whiteboard', 'artifact', session, callId],
    queryFn: async () => (await api.list_artifacts(session, null)).find((a) => a.call_id === callId) ?? null,
  })
  const a = cached ?? listed.data ?? null
  const text = useText(a?.path ?? null, session)
  if (!a && !listed.isPending) return <p className="p-6 text-sm text-fg-secondary">This page is not available on this engine.</p>
  return <HtmlPage callId={callId} html={text} title={title} cap="none" />
}

function ImageBody({ source, blobUrl }: { source: Extract<Source, { t: 'image' }>; blobUrl(key: string): Promise<string | null> }) {
  const [url, setUrl] = useState<string | null>(null)
  useEffect(() => {
    let live = true
    if (source.blob) void blobUrl(source.blob).then((u) => live && setUrl(u))
    return () => {
      live = false
    }
  }, [source.blob, blobUrl])
  if (source.session && source.call_id) return <PageLikeImage session={source.session} callId={source.call_id} />
  return url ? <img src={url} alt="" className="mx-auto max-h-full max-w-full object-contain" /> : <p className="p-6 text-sm text-fg-secondary">{source.path ?? 'Image'}</p>
}

function PageLikeImage({ session, callId }: { session: string; callId: string }) {
  const a = useSessions((s) => s.sessions[session]?.artifacts.get(callId))
  return <p className="p-6 text-sm text-fg-secondary">{a?.title ?? a?.name ?? 'Image artifact'}</p>
}

const fence = (code: string, language: string) => ({ code, isIncomplete: false, language }) as unknown as CustomRendererProps

function Body({ card, source, blobUrl, onSession }: { card: Card; source: Source | undefined; blobUrl(key: string): Promise<string | null>; onSession(id: string): void }) {
  if (card.placeholder !== undefined || !source) return <p className="p-6 text-sm text-fg-secondary">The agent is still making this card.</p>
  switch (source.t) {
    case 'note':
      return <NoteView id={source.doc} embedded hidePopout />
    case 'page':
      return <PageBody session={source.session} callId={source.call_id} title={card.title} />
    case 'diagram':
      return (
        <div className="p-4">
          {source.lang === 'mermaid' ? <MermaidFence {...fence(source.code, 'mermaid')} fill /> : source.lang === 'openui' ? <OpenUIFence {...fence(source.code, 'openui')} /> : <JsonRenderFence {...fence(source.code, 'json-render')} />}
        </div>
      )
    case 'excerpt':
      return (
        <div className="p-4">
          <Excerpt path={source.path} text={source.text} start={source.start} lang={source.lang} full />
        </div>
      )
    case 'diff':
      return <DiffBody session={source.frozen?.session ?? source.session} spec={source.spec as DiffSpec} className="p-2" />
    case 'terminal':
      return <Terminal session={source.session} job={source.job} label={card.title} className="h-full" />
    case 'web':
      // 218 review B1: a web card's URL is agent-writable; `WebSheetBody` shows only http(s) with a dotted host. Most
      // sites refuse framing and the app's CSP `frame-src` admits no http origin, so the page runs in a native web view.
      return <WebSheetBody card={card} url={source.url} />
    case 'image':
      return <ImageBody source={source} blobUrl={blobUrl} />
    case 'sticky':
      return <p className="whitespace-pre-wrap p-6 text-base">{source.text}</p>
    case 'session':
      return (
        <div className="p-6">
          <button type="button" onClick={() => onSession(source.session)} className="flex items-center gap-2 rounded-lg bg-layer-2 px-3 py-2 text-sm">
            <MessageSquare className="size-4" /> Open the session
          </button>
        </div>
      )
    case 'view':
      return (
        <div className="relative h-full">
          <Suspense fallback={null}>
            <ViewCard path={source.path} interactive />
          </Suspense>
        </div>
      )
    case 'task':
      // The task's own detail page, live and interactive (the sheet is the phone's and the outline's Open).
      return (
        <div className="relative h-full">
          <Suspense fallback={null}>
            <ViewCard path={taskPath(source.task)} interactive />
          </Suspense>
        </div>
      )
    case 'deck':
      return (
        <div className="relative h-full">
          <Suspense fallback={null}>
            <DeckSheetView deckId={source.deck} section={source.section} snapshotId={source.snapshot_id} />
          </Suspense>
        </div>
      )
  }
}

export function CardSheet({ card, phone, blobUrl, onClose, onSession }: { card: Card; phone: boolean; blobUrl(key: string): Promise<string | null>; onClose(): void; onSession(id: string): void }) {
  const source = versionOf(card)?.source
  return (
    <div
      role="dialog"
      aria-label={`${card.title || 'Card'} — ${KIND_LABEL[card.kind]}`}
      className={`wb-slide absolute inset-y-0 right-0 z-40 flex flex-col border-l border-border-subtle bg-background shadow-2xl ${phone ? 'w-full' : 'w-[60%] min-w-[360px]'}`}
      onKeyDown={(e) => {
        if (e.key === 'Escape') {
          e.stopPropagation()
          onClose()
        }
      }}
    >
      <header className="flex shrink-0 items-center gap-2 border-b border-border-subtle px-3 pt-[calc(var(--safe-top,0px)+8px)] pb-2">
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">{card.title || 'Untitled'}</h2>
        <span className="text-xs text-fg-tertiary">{KIND_LABEL[card.kind]}</span>
        <button type="button" aria-label="Close" onClick={onClose} className="grid size-9 place-items-center rounded-full hover:bg-layer-2" autoFocus>
          <X className="size-4" />
        </button>
      </header>
      <div className="min-h-0 flex-1 overflow-auto">
        <Body card={card} source={source} blobUrl={blobUrl} onSession={onSession} />
      </div>
    </div>
  )
}
