import { openUrl } from '@tauri-apps/plugin-opener'
import { Download, X } from 'lucide-react'
import { type CSSProperties, type MouseEvent, type PointerEvent, useEffect, useMemo, useRef, useState } from 'react'
import { toast } from 'sonner'
import { Streamdown } from 'streamdown'
import { Button } from '@/components/ui/button'
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from '@/components/ui/select'
import { Sheet, SheetClose, SheetContent, SheetDescription, SheetTitle } from '@/components/ui/sheet'
import { parseCsv } from '@/lib/csv'
import { extOf, fmtBytes, isLocal } from '@/lib/files'
import { useApi } from '@/lib/rpc'
import { useHarnesses } from '@/link/harnesses'
import type { Artifact } from '@/rpc/extra'
import { on, useSessionField, useSessions } from '@/store/sessions'
import { ArtifactMenu, RevealButton, saveAs, useText } from './ArtifactCard'
import { Img, useFileUrl } from './Attachments'
import { plugins, shikiTheme } from './MessageAssistant'

const MIN_W = 360
const DEFAULT_W = 640

/** Prepended to HTML artifacts: no request leaves the frame, even for an `<img src=https://…>` (35 §9). */
const CSP = '<meta http-equiv="Content-Security-Policy" content="default-src \'none\'; style-src \'unsafe-inline\'; img-src data:">'

const MD_CLASS =
  '[&_h1]:mb-2 [&_h1]:text-2xl [&_h1]:font-semibold [&_h2]:mb-2 [&_h2]:mt-6 [&_h2]:text-xl [&_h2]:font-semibold [&_h3]:mt-4 [&_h3]:font-semibold [&_p]:my-1 [&_p]:leading-[26px] [&_li]:pt-2 [&_a]:underline [&_a]:underline-offset-2 [&_pre]:rounded-2xl [&_table_th]:font-semibold'

/** `text` fenced with the extension as language; the fence outruns any backtick run inside. */
function fenced(text: string, lang: string): string {
  const longest = Math.max(2, ...[...text.matchAll(/`+/g)].map((m) => m[0].length))
  const fence = '`'.repeat(longest + 1)
  return `${fence}${lang}\n${text}\n${fence}`
}

/** Committed markdown through streamdown; `http(s)` links open with the OS. */
function Markdown({ text }: { text: string }) {
  const onClick = (e: MouseEvent<HTMLDivElement>) => {
    const href = e.target instanceof Element ? e.target.closest('a')?.getAttribute('href') : null
    if (!href || href.startsWith('#')) return
    e.preventDefault()
    if (/^https?:/.test(href)) openUrl(href).catch((err: unknown) => toast.error(String(err)))
  }
  return (
    <div onClickCapture={onClick} className="px-6 py-4">
      <Streamdown mode="static" plugins={plugins} shikiTheme={shikiTheme} linkSafety={{ enabled: false }} className={MD_CLASS}>
        {text}
      </Streamdown>
    </div>
  )
}

/** The parsed CSV as a table: sticky header, first 500 rows. */
function Csv({ text, tsv }: { text: string; tsv: boolean }) {
  const table = useMemo(() => parseCsv(text, tsv), [text, tsv])
  return (
    <div className="flex h-full flex-col">
      <div className="min-h-0 flex-1 overflow-auto">
        <table className="w-full border-collapse font-mono text-xs">
          <thead className="sticky top-0 bg-elevated shadow-[inset_0_-1px_0_var(--border-default)]">
            <tr>
              {table.header.map((h, i) => (
                <th key={i} className="px-3 py-2 text-left font-semibold whitespace-nowrap">
                  {h}
                </th>
              ))}
            </tr>
          </thead>
          <tbody>
            {table.rows.map((row, r) => (
              <tr key={r} className="border-t border-border-subtle">
                {row.map((cell, c) => (
                  <td key={c} className="px-3 py-1.5 whitespace-nowrap">
                    {cell}
                  </td>
                ))}
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <div className="border-t border-border-subtle px-4 py-2 text-xs text-fg-tertiary">
        {table.rows.length < table.total ? `showing ${table.rows.length} of ${table.total} rows` : `${table.total} rows`}
      </div>
    </div>
  )
}

/** `<iframe src={url}>` of the blob: WKWebView's own PDF viewer. */
function Pdf({ a }: { a: Artifact }) {
  const url = useFileUrl(a.path)
  if (!url) return <div className="m-6 h-64 animate-pulse rounded-xl bg-layer-2" />
  return <iframe src={url} title={a.name} className="h-full w-full border-0" />
}

/** The viewer for one artifact by kind (35 §3.2). */
function Body({ a }: { a: Artifact }) {
  const api = useApi()
  const active = useHarnesses((s) => s.active)
  const textual = a.kind === 'markdown' || a.kind === 'text' || a.kind === 'csv' || a.kind === 'html'
  const text = useText(textual ? a.path : null)
  if (a.kind === 'image' || a.kind === 'svg') {
    return (
      <div className="flex h-full items-start justify-center overflow-auto p-6">
        <Img path={a.path} className="max-w-full rounded-lg" />
      </div>
    )
  }
  if (a.kind === 'pdf') return <Pdf a={a} />
  if (a.kind === 'file') {
    return (
      <div className="flex h-full flex-col items-center justify-center gap-3 text-fg-secondary">
        <span className="text-sm">{a.mime}</span>
        <Button variant="outline" onClick={() => saveAs(api, a, isLocal(active)).catch((e: unknown) => toast.error(String(e)))}>
          <Download />
          {isLocal(active) ? 'Save as…' : 'Download'}
        </Button>
      </div>
    )
  }
  if (text === null) return <div className="m-6 h-64 animate-pulse rounded-xl bg-layer-2" />
  if (text === '') return <div className="p-6 text-sm text-danger">could not load {a.name}</div>
  switch (a.kind) {
    case 'markdown':
      return <Markdown text={text} />
    case 'text':
      return (
        <div className="p-4 [&_pre]:text-xs">
          <Streamdown mode="static" plugins={plugins} shikiTheme={shikiTheme}>
            {fenced(text, extOf(a.name))}
          </Streamdown>
        </div>
      )
    case 'csv':
      return <Csv text={text} tsv={extOf(a.name) === 'tsv'} />
    default:
      return <iframe sandbox="" srcDoc={CSP + text} title={a.name} className="h-full w-full border-0 bg-white" />
  }
}

/** Header + body for the artifact at `callId`; a version `Select` swaps the viewer among same-name artifacts of the session. */
function Content({ session, callId }: { session: string; callId: string }) {
  const artifacts = useSessionField(session, 'artifacts')
  const [view, setView] = useState(callId)
  const shown = artifacts?.get(view) ?? artifacts?.get(callId)
  const versions = useMemo(() => {
    if (!shown || !artifacts) return []
    return [...artifacts.values()].filter((a) => a.name === shown.name).sort((x, y) => y.version - x.version)
  }, [artifacts, shown])
  if (!shown) return null
  return (
    <>
      <div className="flex items-start gap-2 border-b border-border-subtle px-4 pt-3 pb-3 pr-3">
        <div className="min-w-0 flex-1">
          <SheetTitle className="truncate text-base">{shown.title}</SheetTitle>
          <SheetDescription className="truncate text-xs">
            {shown.name} · {shown.kind.toUpperCase()} · {fmtBytes(shown.bytes)}
          </SheetDescription>
        </div>
        {versions.length > 1 && (
          <Select value={shown.call_id} onValueChange={setView}>
            <SelectTrigger size="sm" className="h-8 shrink-0 text-xs" aria-label="Version">
              <SelectValue />
            </SelectTrigger>
            <SelectContent align="end">
              {versions.map((v) => (
                <SelectItem key={v.call_id} value={v.call_id} className="text-xs">
                  v{v.version}
                </SelectItem>
              ))}
            </SelectContent>
          </Select>
        )}
        <RevealButton artifact={shown} />
        <ArtifactMenu artifact={shown} />
        <SheetClose asChild>
          <Button variant="ghost" size="icon-sm" className="text-fg-secondary" aria-label="Close">
            <X className="size-4" />
          </Button>
        </SheetClose>
      </div>
      <div className="min-h-0 flex-1 overflow-auto">
        <Body a={shown} />
      </div>
    </>
  )
}

/**
 * Right-side sheet showing one artifact (35 §7.2). Opens itself for the first
 * `ArtifactShown` of a turn while the window is focused; closing it suppresses
 * auto-open for the rest of that turn.
 */
export function ArtifactPanel({ session }: { session: string }) {
  const callId = useSessionField(session, 'artifactPanel') ?? null
  const [width, setWidth] = useState(DEFAULT_W)
  /** `turnSeq` of the turn whose first artifact already opened (or was dismissed). */
  const handled = useRef<number | undefined>(undefined)

  useEffect(
    () =>
      on('ArtifactShown', (e) => {
        if (e.session !== session) return
        const turn = useSessions.getState().sessions[session]?.turnSeq
        if (handled.current === turn) return
        handled.current = turn
        if (document.hasFocus()) useSessions.getState().setArtifactPanel(session, e.call_id)
      }),
    [session],
  )

  const close = () => {
    handled.current = useSessions.getState().sessions[session]?.turnSeq
    useSessions.getState().setArtifactPanel(session, null)
  }

  // ponytail: plain pointer events on a 6 px strip; react-resizable-panels if a second panel ever needs it.
  const drag = (e: PointerEvent<HTMLDivElement>) => {
    e.currentTarget.setPointerCapture(e.pointerId)
    const move = (ev: globalThis.PointerEvent) =>
      setWidth(Math.min(Math.max(MIN_W, window.innerWidth - ev.clientX), window.innerWidth / 2))
    const up = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', up)
    }
    window.addEventListener('pointermove', move)
    window.addEventListener('pointerup', up)
  }

  return (
    <Sheet open={callId !== null} onOpenChange={(o) => !o && close()}>
      <SheetContent
        showCloseButton={false}
        className="w-full gap-0 p-0 transition-transform sm:w-(--panel-w) sm:max-w-[50vw]"
        style={{ '--panel-w': `${width}px` } as CSSProperties}
      >
        <div
          className="absolute inset-y-0 left-0 z-10 hidden w-1.5 cursor-col-resize hover:bg-border-strong sm:block"
          onPointerDown={drag}
        />
        {callId && <Content key={callId} session={session} callId={callId} />}
      </SheetContent>
    </Sheet>
  )
}
