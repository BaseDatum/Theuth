import { useNavigate } from '@tanstack/react-router'
import { AppWindow, BookOpen, Copy, Download, ExternalLink, File as FileIcon, FileSearch, FileText, type LucideIcon, MessageSquare, Mic, NotebookText, Paperclip, Presentation, StickyNote, Terminal, X } from 'lucide-react'
import { type DragEvent, useEffect, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { openViewTile } from '@/components/tiles/open'
import { Button } from '@/components/ui/button'
import { Dialog, DialogClose, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { type AttachRef, DRAG_TYPE, KIND_LABEL, type Kind, attachTo } from '@/lib/attach'
import { extOf, readBytes, resolveDocument, resolveFile, saveBlob } from '@/lib/files'
import { tileLeaf, usePanes } from '@/lib/panes'
import { container, touch } from '@/lib/platform'
import { type PopoutTarget, canPopout, openPopout } from '@/lib/popout'
import { cn } from '@/lib/utils'
import { useEngines } from '@/link/engines'
import type { Api } from '@/rpc/bindings/client'
import { useConnection } from '@/store/connection'
import { terminalPath } from '@/store/terminals'
import { useScopedDisclosure } from './disclosure'

/** §7.5 icons; an artifact chip has no record, so it takes the generic file glyph (picker rows refine by artifact kind). */
export const KIND_ICON: Record<Kind, LucideIcon> = {
  note: StickyNote,
  knowledge: BookOpen,
  artifact: FileText,
  session: MessageSquare,
  meeting: Mic,
  notebook: NotebookText,
  source: FileSearch,
  whiteboard: Presentation,
  file: FileIcon,
  terminal: Terminal,
}

const IMAGE_EXT = new Set(['png', 'jpg', 'jpeg', 'gif', 'webp', 'bmp'])

function fileExt(ref: AttachRef): string {
  return extOf(ref.title) || extOf(ref.id)
}

function isImageFile(ref: AttachRef): boolean {
  return IMAGE_EXT.has(fileExt(ref))
}

function isPdfFile(ref: AttachRef): boolean {
  return fileExt(ref) === 'pdf'
}

/** Save any attached file (including extensionless or arbitrary binary files) without extension restrictions. */
async function downloadFileRef(api: Api, ref: AttachRef): Promise<void> {
  const bytes = await readBytes(api, ref.id)
  const blob = new Blob([bytes])
  const name = ref.title || ref.id.slice(ref.id.lastIndexOf('/') + 1) || 'file'
  const ext = fileExt(ref)
  if (!ext && (container() === 'web' || container() === 'ios')) {
    const a = document.createElement('a')
    a.href = URL.createObjectURL(blob)
    a.download = name
    a.click()
    URL.revokeObjectURL(a.href)
    return
  }
  const stem = ext && name.toLowerCase().endsWith(`.${ext}`) ? name.slice(0, -(ext.length + 1)) : name
  await saveBlob(api, blob, stem || 'file', ext)
}

function openFileInWindow(ref: AttachRef): void {
  const api = useConnection.getState().api
  if (!api) {
    toast.error('Not connected')
    return
  }
  const fail = (e: unknown) => toast.error(e instanceof Error ? e.message : String(e))
  if (isPdfFile(ref)) {
    void resolveDocument(api, ref.id).then((url) => {
      window.open(url, '_blank', 'noopener')
    }, fail)
    return
  }
  if (isImageFile(ref)) {
    const active = ref.id.startsWith('/') ? useEngines.getState().active : ''
    void Promise.resolve(resolveFile(api, ref.id, active)).then((url) => {
      window.open(url, '_blank', 'noopener')
    }, fail)
    return
  }
  void downloadFileRef(api, ref).catch(fail)
}

function parseTerminalId(id: string): { engine: string; shell: string } | null {
  const sep = id.indexOf(':')
  if (sep <= 0 || sep === id.length - 1) return null
  return { engine: id.slice(0, sep), shell: id.slice(sep + 1) }
}

/** Props that make a row a drag source for the composer's drop (§7.8); desktop only — no HTML5 drag on iOS Safari. */
export function dragSource(ref: () => AttachRef): { draggable: boolean; onDragStart(e: DragEvent): void } {
  return {
    draggable: !touch(),
    onDragStart(e) {
      e.dataTransfer.setData(DRAG_TYPE, JSON.stringify(ref()))
      e.dataTransfer.effectAllowed = 'copy'
    },
  }
}

/** Navigate to the thing a reference points at (§7.6); a knowledge `id@commit` opens 49's historical mode. */
export function useOpenRef(): (ref: AttachRef) => void {
  const navigate = useNavigate()
  return (ref) => {
    switch (ref.kind) {
      case 'note':
        return void navigate({ to: '/notes/$id', params: { id: ref.id }, search: {} })
      case 'knowledge': {
        const at = ref.id.indexOf('@')
        return void navigate({
          to: '/knowledge/$',
          params: { _splat: at < 0 ? ref.id : ref.id.slice(0, at) },
          search: at < 0 ? {} : { change: ref.id.slice(at + 1) },
        })
      }
      case 'artifact':
        return void navigate({ to: '/artifacts/$callId', params: { callId: ref.id } })
      case 'session':
        return void navigate({ to: '/c/$session', params: { session: ref.id }, search: {} })
      case 'meeting':
        return void navigate({ to: '/meetings/$id', params: { id: ref.id } })
      case 'notebook':
        return void navigate({ to: '/notebooks/$id', params: { id: ref.id }, search: {} })
      case 'source':
        return void navigate({ to: '/sources/$id', params: { id: ref.id }, search: {} })
      case 'whiteboard':
        return void navigate({ to: '/whiteboards/$id', params: { id: ref.id }, search: {} })
      case 'file':
        return openFileInWindow(ref)
      case 'terminal': {
        const parsed = parseTerminalId(ref.id)
        if (!parsed) return
        const id = openViewTile({ path: terminalPath({ ...parsed, name: ref.title }), title: ref.title })
        if (id !== null) {
          usePanes.getState().select(tileLeaf(id))
          return
        }
        return void navigate({ to: '/terminal/$engine/$shell', params: parsed, search: { name: ref.title } })
      }
    }
  }
}

/** The window a reference can open alone (`lib/popout`), `null` for knowledge (no popped route). */
export function popoutOf(ref: AttachRef): PopoutTarget | null {
  switch (ref.kind) {
    case 'note':
      return { kind: 'note', doc: ref.id }
    case 'artifact':
      return { kind: 'artifact', callId: ref.id }
    case 'session':
      return { kind: 'chat', session: ref.id }
    case 'meeting':
      return { kind: 'meeting', meeting: ref.id }
    case 'terminal': {
      const parsed = parseTerminalId(ref.id)
      return parsed ? { kind: 'node-terminal', engine: parsed.engine, shell: parsed.shell, name: ref.title } : null
    }
    case 'whiteboard':
    case 'knowledge':
    case 'notebook':
    case 'source':
    case 'file':
      return null
  }
}

/** Inline lightbox for image and PDF `'file'` attachments; loads lazily only while open. */
function FileLightbox({ item, open, onOpenChange }: { item: AttachRef; open: boolean; onOpenChange(v: boolean): void }) {
  const image = isImageFile(item)
  const pdf = isPdfFile(item)
  const [src, setSrc] = useState<string | null>(null)
  useEffect(() => {
    if (!open) return
    const api = useConnection.getState().api
    if (!api) {
      setSrc('')
      return
    }
    let live = true
    let ownedUrl = ''
    setSrc(null)
    if (pdf) {
      resolveDocument(api, item.id).then(
        (u) => {
          if (!live) return URL.revokeObjectURL(u)
          ownedUrl = u
          setSrc(u)
        },
        () => live && setSrc(''),
      )
    } else if (image) {
      const active = item.id.startsWith('/') ? useEngines.getState().active : ''
      Promise.resolve(resolveFile(api, item.id, active)).then(
        (u) => live && setSrc(u),
        () => live && setSrc(''),
      )
    }
    return () => {
      live = false
      if (ownedUrl) URL.revokeObjectURL(ownedUrl)
    }
  }, [open, item.id, image, pdf])
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent
        className="inset-0 flex h-dvh w-screen max-w-none translate-x-0 translate-y-0 items-center justify-center gap-0 rounded-none border-0 bg-transparent p-0 shadow-none sm:max-w-none"
        showCloseButton={false}
        onClick={(e) => e.target === e.currentTarget && onOpenChange(false)}
      >
        <DialogTitle className="sr-only">{item.title}</DialogTitle>
        <DialogClose asChild>
          <Button
            variant="ghost"
            size="icon"
            aria-label="Close"
            className="absolute top-[max(0.75rem,env(safe-area-inset-top))] right-[max(0.75rem,env(safe-area-inset-right))] size-11 rounded-full bg-layer-1/80 text-fg-primary shadow-sm hover:bg-layer-1 [&_svg]:size-5"
          >
            <X />
          </Button>
        </DialogClose>
        <div className="relative flex flex-col items-center gap-2">
          {src === null && <div className="h-64 w-96 max-w-full animate-pulse rounded-xl bg-layer-2" />}
          {src === '' && <div className="rounded-xl bg-layer-1 px-4 py-3 text-sm text-fg-secondary">Preview unavailable: {item.title}</div>}
          {src && image && (
            <img
              src={src}
              alt={item.title}
              className="max-h-[calc(100dvh_-_8rem_-_env(safe-area-inset-top)_-_env(safe-area-inset-bottom))] max-w-[calc(100vw_-_2rem_-_env(safe-area-inset-left)_-_env(safe-area-inset-right))] w-auto rounded-xl object-contain"
            />
          )}
          {src && pdf && (
            <iframe
              src={src}
              title={item.title}
              className="h-[calc(100dvh_-_8rem_-_env(safe-area-inset-top)_-_env(safe-area-inset-bottom))] w-[min(90vw,1100px)] rounded-xl border-0 bg-white"
            />
          )}
        </div>
      </DialogContent>
    </Dialog>
  )
}

/**
 * One attached reference as a pill: kind icon, title, muted kind. Click opens a small popover with the subtitle,
 * **Open** and — where the container can and the kind has a popped route — **Pop out**; `again` (a history chip,
 * §7.9) adds **Attach again** (a chip in `again`'s composer) and **Copy id**; `onRemove` puts a × on the pill.
 */
export function AttachChip({ item: ref, again, onRemove, className }: { item: AttachRef; again?: string | null; onRemove?: () => void; className?: string }) {
  const popout = popoutOf(ref)
  const open = useOpenRef()
  const Icon = KIND_ICON[ref.kind] ?? Paperclip
  const label = KIND_LABEL[ref.kind] ?? ref.kind
  const [shown, setShown] = useScopedDisclosure(`attach|${ref.kind}|${ref.id}`)
  const [previewOpen, setPreviewOpen] = useScopedDisclosure(`attach-preview|${ref.kind}|${ref.id}`)
  const previewableFile = ref.kind === 'file' && (isImageFile(ref) || isPdfFile(ref))
  const onOpenClick = () => {
    setShown(false)
    if (previewableFile) {
      setPreviewOpen(true)
      return
    }
    open(ref)
  }
  const onDownloadClick = () => {
    setShown(false)
    const api = useConnection.getState().api
    if (!api) {
      toast.error('Not connected')
      return
    }
    void downloadFileRef(api, ref).catch((e: unknown) => toast.error(e instanceof Error ? e.message : String(e)))
  }
  return (
    <>
      <Popover open={shown} onOpenChange={setShown}>
        <PopoverTrigger asChild>
          <button
            type="button"
            className={cn(
              'flex h-7 max-w-[240px] items-center gap-1 rounded-full bg-layer-2 px-2 text-xs hover:bg-layer-3 pointer-coarse:h-8',
              className,
            )}
            aria-label={`${label}: ${ref.title}`}
          >
            <Icon className="size-3.5 shrink-0 text-fg-secondary" />
            <span className="truncate">{ref.title}</span>
            <span className="shrink-0 text-fg-tertiary">{label}</span>
            {onRemove && (
              <span
                role="button"
                tabIndex={0}
                aria-label={`Remove ${ref.title}`}
                className="-mr-1 ml-0.5 flex size-5 shrink-0 items-center justify-center rounded-full text-fg-tertiary hover:bg-layer-3 hover:text-fg pointer-coarse:size-6"
                onClick={(e) => {
                  e.stopPropagation()
                  onRemove()
                }}
                onKeyDown={(e) => {
                  if (e.key === 'Enter' || e.key === ' ') {
                    e.preventDefault()
                    e.stopPropagation()
                    onRemove()
                  }
                }}
              >
                <X className="size-3" />
              </span>
            )}
          </button>
        </PopoverTrigger>
        <PopoverContent align="start" className="w-auto max-w-80 p-2 text-sm">
          <div className="px-1 pb-1">
            <div className="font-medium break-words">{ref.title}</div>
            {ref.meta && <div className="text-xs text-fg-secondary break-words">{ref.meta}</div>}
          </div>
          <div className="flex flex-wrap gap-1">
            {ref.kind === 'file' ? (
              <>
                {previewableFile && (
                  <Button variant="ghost" size="sm" onClick={onOpenClick}>
                    <ExternalLink /> Preview
                  </Button>
                )}
                <Button variant="ghost" size="sm" onClick={onDownloadClick}>
                  <Download /> {previewableFile ? 'Save' : 'Download'}
                </Button>
              </>
            ) : (
              <Button variant="ghost" size="sm" onClick={onOpenClick}>
                <ExternalLink /> Open
              </Button>
            )}
            {popout && canPopout() && (
              <Button variant="ghost" size="sm" onClick={() => void openPopout(popout)}>
                <AppWindow /> Pop out
              </Button>
            )}
            {again !== undefined && (
              <Button variant="ghost" size="sm" onClick={() => attachTo(again, ref)}>
                <Paperclip /> Attach again
              </Button>
            )}
            {again !== undefined && (
              <Button
                variant="ghost"
                size="sm"
                onClick={() => navigator.clipboard.writeText(ref.id).then(() => toast('Copied'), () => toast.error('Copy failed'))}
              >
                <Copy /> Copy id
              </Button>
            )}
          </div>
        </PopoverContent>
      </Popover>
      {previewableFile && <FileLightbox item={ref} open={previewOpen} onOpenChange={setPreviewOpen} />}
    </>
  )
}
