import { save } from '@tauri-apps/plugin-dialog'
import { openPath, revealItemInDir } from '@tauri-apps/plugin-opener'
import { useNavigate } from '@tanstack/react-router'
import { File, FileCode, FileText, FolderOpen, Image as ImageIcon, Loader2, MoreHorizontal, PanelRight, Table } from 'lucide-react'
import { memo, type MouseEvent, type ReactNode, useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { fmtBytes, isLocal, readText, resolveFile } from '@/lib/files'
import { resultLine } from '@/lib/describe'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { useHarnesses } from '@/link/harnesses'
import type { Api } from '@/rpc/api'
import type { Artifact, Kind } from '@/rpc/extra'
import { useConnection } from '@/store/connection'
import { type Entry, useSessions } from '@/store/sessions'
import { Img } from './Attachments'

type Tool = Extract<Entry, { kind: 'tool' }>

/** Kinds whose bytes are UTF-8 text: previewed as lines, copied as text. */
export const TEXT_KINDS: Record<Kind, boolean> = {
  markdown: true,
  text: true,
  csv: true,
  html: true,
  image: false,
  svg: false,
  pdf: false,
  file: false,
}

/** One lucide glyph per kind; the card slot, the explorer's thumbs and its kind chips share it. */
export const GLYPH: Record<Kind, typeof File> = {
  markdown: FileText,
  text: FileText,
  csv: Table,
  image: ImageIcon,
  svg: ImageIcon,
  pdf: FileText,
  html: FileCode,
  file: File,
}

const fail = (e: unknown) => toast.error(e instanceof Error ? e.message : String(e))

/** Text of a harness file, `null` while loading, `''` on failure. */
export function useText(path: string | null): string | null {
  const api = useApi()
  const [text, setText] = useState<string | null>(null)
  useEffect(() => {
    if (!path) return
    let live = true
    setText(null)
    readText(api, path).then(
      (t) => live && setText(t),
      () => live && setText(''),
    )
    return () => {
      live = false
    }
  }, [api, path])
  return text
}

/** `report.pdf · PDF · 3 pages · 84 KB`; `rows` when the CSV is parsed. */
export function metaLine(a: Artifact, rows?: number): string {
  const parts = [a.name, a.kind.toUpperCase()]
  if (a.pages != null) parts.push(`${a.pages} page${a.pages === 1 ? '' : 's'}`)
  if (rows != null) parts.push(`${rows} row${rows === 1 ? '' : 's'}`)
  parts.push(fmtBytes(a.bytes))
  return parts.join(' · ')
}

/** Save the blob: a native save dialog + `export_artifact` locally; a browser download of the blob URL elsewhere. */
export async function saveAs(api: Api, a: Artifact, local: boolean) {
  if (local) {
    const dest = await save({ defaultPath: a.name })
    if (!dest) return
    await api.export_artifact(a.id, dest)
    toast.success(`Saved ${a.name}`)
    return
  }
  const url = await resolveFile(api, a.path, useHarnesses.getState().active)
  const el = document.createElement('a')
  el.href = url
  el.download = a.name
  el.click()
}

/** `⋯` menu: Copy (text kinds), Save as… / Download, Open (local), Open in explorer (unless `explorer`), Copy path. */
export function ArtifactMenu({ artifact: a, className, explorer }: { artifact: Artifact; className?: string; explorer?: boolean }) {
  const api = useApi()
  const navigate = useNavigate()
  const active = useHarnesses((s) => s.active)
  const local = isLocal(active)
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <Button variant="ghost" size="icon-sm" className={cn('text-fg-secondary', className)} aria-label="More">
          <MoreHorizontal className="size-4" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-48">
        {TEXT_KINDS[a.kind] && (
          <DropdownMenuItem onSelect={() => readText(api, a.path).then((t) => navigator.clipboard.writeText(t), fail)}>
            Copy
          </DropdownMenuItem>
        )}
        <DropdownMenuItem onSelect={() => saveAs(api, a, local).catch(fail)}>{local ? 'Save as…' : 'Download'}</DropdownMenuItem>
        {local && <DropdownMenuItem onSelect={() => openPath(a.path).catch(fail)}>Open</DropdownMenuItem>}
        {!explorer && (
          <DropdownMenuItem onSelect={() => void navigate({ to: '/artifacts/$callId', params: { callId: a.call_id } })}>
            Open in explorer
          </DropdownMenuItem>
        )}
        <DropdownMenuSeparator />
        <DropdownMenuItem onSelect={() => void navigator.clipboard.writeText(a.path)}>Copy path</DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/** Reveal the blob in the OS file manager; local harness only (35 §7.4). */
export function RevealButton({ artifact: a }: { artifact: Artifact }) {
  const active = useHarnesses((s) => s.active)
  const platform = useConnection((s) => s.info?.platform)
  if (!isLocal(active)) return null
  const label = platform === 'macos' ? 'Reveal in Finder' : 'Show in file manager'
  return (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button
          variant="ghost"
          size="icon-sm"
          className="text-fg-secondary"
          aria-label={label}
          onClick={() => revealItemInDir(a.path).catch(fail)}
        >
          <FolderOpen className="size-4" />
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  )
}

/** The spinner row while a call's arguments stream (35 §0.7): `writing report.pdf… 3.2 KB` for a `show` carrying
 * `content`, `showing report.pdf…` for one naming a path, `preparing <tool>… 3.2 KB` for any other tool. */
export function progressLabel(tool: string, p: { name?: string; writing?: boolean; bytes: number }): string {
  const size = fmtBytes(p.bytes)
  if (tool !== 'show') return `preparing ${tool}… ${size}`
  if (p.writing) return p.name ? `writing ${p.name}… ${size}` : `writing… ${size}`
  return p.name ? `showing ${p.name}…` : 'preparing show…'
}

/** Progress label plus a thin indeterminate bar. */
export function Progress({ tool, progress }: { tool: string; progress: { name?: string; writing?: boolean; bytes: number } }) {
  return (
    <div className="mt-1 flex w-full flex-col gap-1.5">
      <span className="text-xs text-fg-tertiary">{progressLabel(tool, progress)}</span>
      <div className="h-0.5 w-full overflow-hidden rounded-full bg-layer-3">
        <div className="h-full w-1/3 animate-indeterminate rounded-full bg-fg-accent" />
      </div>
    </div>
  )
}

/** The 56×72 slot: preview PNG, the image itself, the first 6 lines, or a kind glyph. */
function Preview({ a, text }: { a: Artifact; text: string | null }) {
  const box = 'h-[72px] w-14 shrink-0 overflow-hidden rounded-lg bg-layer-2'
  if (a.preview) return <Img path={a.preview} className={cn(box, 'object-cover object-top')} />
  if (a.kind === 'image' || a.kind === 'svg') return <Img path={a.path} className={cn(box, 'object-cover')} />
  if (TEXT_KINDS[a.kind] && a.kind !== 'html' && text) {
    return (
      <pre className={cn(box, 'p-1.5 font-mono text-[10px] leading-3 whitespace-pre text-fg-tertiary')}>
        {text.split('\n', 6).join('\n')}
      </pre>
    )
  }
  const Glyph = GLYPH[a.kind]
  return (
    <div className={cn(box, 'flex items-center justify-center text-fg-tertiary')}>
      <Glyph className="size-6" />
    </div>
  )
}

/** The card frame — the same box as every other tool card (`MessageTool`): `⚙ show` header, then the preview
 * slot on the left and the body on the right; `onClick` opens the panel. */
function Frame({ children, slot, onClick, busy }: { children: ReactNode; slot: ReactNode; onClick?: () => void; busy?: boolean }) {
  const click = (e: MouseEvent<HTMLDivElement>) => {
    // Buttons and menus inside the card keep their own actions.
    if (e.target instanceof Element && e.target.closest('button,a,[role=menu]')) return
    onClick?.()
  }
  return (
    <div
      role={onClick ? 'button' : undefined}
      className={cn(
        'my-2 min-w-0 rounded-xl border border-border-subtle bg-layer-1 px-4 py-3 text-sm',
        onClick && 'cursor-pointer hover:border-border-strong',
      )}
      onClick={click}
    >
      <div className="flex items-center gap-2 font-medium">
        <span className="shrink-0 text-fg-secondary">⚙</span>
        <span>show</span>
        {busy && <Loader2 className="size-3.5 shrink-0 animate-spin text-fg-tertiary" />}
      </div>
      <div className="mt-2 flex min-w-0 gap-3">
        {slot}
        <div className="flex min-w-0 flex-1 flex-col gap-1">{children}</div>
      </div>
    </div>
  )
}

const SKELETON = <div className="h-[72px] w-14 shrink-0 animate-pulse rounded-lg bg-layer-2" />
const FAILED_SLOT = (
  <div className="flex h-[72px] w-14 shrink-0 items-center justify-center rounded-lg bg-danger-bg text-danger">
    <File className="size-6" />
  </div>
)

/** A `show` tool card (35 §7.1): writing → compiling → shown, or failed with the first diagnostic. */
export const ArtifactCard = memo(function ArtifactCard({ tool, session }: { tool: Tool; session: string }) {
  const artifact = useSessions((s) => s.sessions[session]?.artifacts.get(tool.callId))
  const progress = useSessions((s) => s.sessions[session]?.progress.get(tool.callId))
  const [errors, setErrors] = useState(false)
  const preview = artifact && TEXT_KINDS[artifact.kind] && artifact.kind !== 'html' ? artifact.path : null
  const text = useText(preview)
  const { result } = tool

  if (artifact) {
    const rows = artifact.kind === 'csv' && text ? Math.max(0, text.trimEnd().split('\n').length - 1) : undefined
    const open = () => useSessions.getState().setArtifactPanel(session, tool.callId)
    return (
      <Frame slot={<Preview a={artifact} text={text} />} onClick={open}>
        <div className="flex items-start gap-2">
          <div className="min-w-0 flex-1">
            <div className="truncate font-medium">{artifact.title}</div>
            <div className="truncate text-xs text-fg-secondary">{metaLine(artifact, rows)}</div>
          </div>
          {artifact.version > 1 && (
            <span className="shrink-0 rounded-md bg-layer-2 px-1.5 py-0.5 text-[11px] text-fg-secondary">v{artifact.version}</span>
          )}
        </div>
        <div className="mt-auto flex items-center justify-end gap-0.5">
          <RevealButton artifact={artifact} />
          <Tooltip>
            <TooltipTrigger asChild>
              <Button variant="ghost" size="icon-sm" className="text-fg-secondary" aria-label="Open in panel" onClick={open}>
                <PanelRight className="size-4" />
              </Button>
            </TooltipTrigger>
            <TooltipContent>Open in panel</TooltipContent>
          </Tooltip>
          <ArtifactMenu artifact={artifact} />
        </div>
      </Frame>
    )
  }

  if (result?.isError) {
    const [first = '', ...rest] = result.output.split('\n')
    return (
      <Frame slot={FAILED_SLOT}>
        <div className="rounded-lg bg-danger-bg px-3 py-2 text-danger">
          <div className="font-mono text-xs wrap-anywhere">{resultLine(first)}</div>
          {rest.length > 0 && (
            <>
              <button type="button" className="mt-1 text-xs underline" onClick={() => setErrors((v) => !v)}>
                {errors ? 'Hide errors' : 'Show errors'}
              </button>
              {errors && <pre className="mt-1 font-mono text-xs whitespace-pre-wrap">{rest.join('\n')}</pre>}
            </>
          )}
        </div>
      </Frame>
    )
  }

  // Writing (arguments streaming) or compiling (started, `ArtifactShown` not yet); a finished call whose artifact the list did not carry shows its receipt.
  return (
    <Frame slot={SKELETON} busy={!result}>
      {result ? (
        <div className="truncate font-mono text-xs text-fg-secondary">{resultLine(result.output)}</div>
      ) : tool.pending ? (
        <Progress tool={tool.name} progress={progress ?? { bytes: 0 }} />
      ) : (
        <>
          <div className="h-4 w-1/2 animate-pulse rounded bg-layer-2" />
          <span className="text-xs text-fg-tertiary">compiling…</span>
        </>
      )}
    </Frame>
  )
})
