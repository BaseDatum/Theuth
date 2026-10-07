import { X } from 'lucide-react'
import { memo, type ReactNode, useEffect, useState } from 'react'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { resolveDocument, resolveFile } from '@/lib/files'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { useEngines } from '@/link/engines'
import type { ContextAttachment } from '@/rpc/bindings/ContextAttachment'
import { RemoveButton } from './MessageAssistant'

const IMAGE = /\.(png|jpe?g|gif|webp|bmp|svg)$/i

/** A URL for an engine path via `resolveFile` (31 §9): immediate on macOS local; elsewhere `null` until the bytes arrive, `''` on failure. */
export function useFileUrl(path: string): string | null {
  const api = useApi()
  const active = useEngines((s) => s.active)
  // The local case is synchronous, so the first paint already has the URL; the RPC case is cached, so the effect's call is the same promise.
  const [src, setSrc] = useState<string | null>(() => {
    const r = resolveFile(api, path, active)
    return typeof r === 'string' ? r : null
  })
  useEffect(() => {
    const r = resolveFile(api, path, active)
    if (typeof r === 'string') {
      setSrc(r)
      return
    }
    let live = true
    setSrc(null)
    r.then(
      (url) => live && setSrc(url),
      () => live && setSrc(''),
    )
    return () => {
      live = false
    }
  }, [api, path, active])
  return src
}

/** A typed `blob:` URL for a document (PDF) frame via `resolveDocument` (120 §9b): `null` until the bytes arrive, `''` on failure; revoked on unmount. */
export function useDocumentUrl(path: string): string | null {
  const api = useApi()
  const [src, setSrc] = useState<string | null>(null)
  useEffect(() => {
    let live = true
    let url = ''
    setSrc(null)
    resolveDocument(api, path).then(
      (u) => {
        if (!live) return URL.revokeObjectURL(u)
        url = u
        setSrc(u)
      },
      () => live && setSrc(''),
    )
    return () => {
      live = false
      if (url) URL.revokeObjectURL(url)
    }
  }, [api, path])
  return src
}

/** `<img>` for an engine path: a dim block until the URL resolves, nothing on failure. */
export function Img({ path, className }: { path: string; className: string }) {
  const src = useFileUrl(path)
  if (src === null) return <div className={cn('h-24 w-32 animate-pulse bg-layer-2', className)} />
  if (src === '') return null
  return <img src={src} alt="" className={className} />
}

/** One thumbnail or chip. An `occurrence` is a stored model-visible part (48 §3.3) and carries the removal target; a bare path is an artifact or optimistic file and has none. */
type Item = { key: string; name: string; path: string | null; image: boolean; pdf: boolean; occurrence?: ContextAttachment }

/** Authoritative occurrences replace `files` outright once present (each its own item, so a path attached twice stays two
 * targets and a rematerialized copy never doubles an optimistic path); bare `files` are only our pre-persistence or artifact paths. */
function items(files: string[] | undefined, occurrences: ContextAttachment[] | undefined): Item[] {
  if (occurrences) {
    return occurrences.map((o, i) => ({
      key: `o${i}`,
      name: o.name,
      path: o.path,
      image: o.mime.startsWith('image/') || (!!o.path && IMAGE.test(o.path)),
      pdf: o.mime === 'application/pdf',
      occurrence: o,
    }))
  }
  return (files ?? []).map((path, i) => ({
    key: `f${i}`,
    name: path.slice(path.lastIndexOf('/') + 1),
    path,
    image: IMAGE.test(path),
    pdf: /\.pdf$/i.test(path),
  }))
}

/** Name chip; the name opens the viewer when there is a file behind it. `control` is the sibling remove button, never nested. */
function Chip({ item, removed, onOpen, control }: { item: Item; removed: boolean; onOpen: () => void; control: ReactNode }) {
  const viewable = !!item.path && (item.image || item.pdf)
  return (
    <span className={cn('flex min-w-0 items-center gap-1 rounded-lg bg-layer-2 pl-2 text-sm', removed ? 'text-fg-tertiary' : 'text-fg-secondary')}>
      {removed && <span className="shrink-0 py-1 italic">Removed from context ·</span>}
      <span className="truncate py-1">{item.name}</span>
      {viewable ? (
        <button type="button" className="min-h-11 shrink-0 px-2 text-xs underline underline-offset-2 hover:text-fg-primary" onClick={onOpen}>
          {removed ? 'View original' : 'View'}
        </button>
      ) : (
        <span className="pr-2" />
      )}
      {control}
    </span>
  )
}

/** Image thumbnail with the zoom button and the sibling corner `X`; a preview that fails to load falls back to the named chip, so the occurrence stays removable. */
function Thumb({ item, onOpen, control, chipControl }: { item: Item; onOpen: () => void; control: ReactNode; chipControl: ReactNode }) {
  const src = useFileUrl(item.path ?? '')
  if (src === '') return <Chip item={item} removed={false} onOpen={onOpen} control={chipControl} />
  return (
    <div className="group relative">
      <button type="button" className="cursor-zoom-in" aria-label={`Open image: ${item.name}`} onClick={onOpen}>
        {src === null ? <div className="h-24 w-32 animate-pulse rounded-xl bg-layer-2" /> : <img src={src} alt="" className="max-h-64 rounded-xl" />}
      </button>
      {control}
    </div>
  )
}

/** The lightbox body: the image, or the PDF in a frame (WKWebView's own viewer; iOS draws page 1 — ponytail: reuse the artifact panel's pdf.js pages if that matters). */
function Original({ item }: { item: Item }) {
  return item.image ? <OriginalImage item={item} /> : <OriginalDocument item={item} />
}

function OriginalImage({ item }: { item: Item }) {
  const src = useFileUrl(item.path ?? '')
  if (src === null) return <div className="h-64 w-96 animate-pulse rounded-xl bg-layer-2" />
  if (src === '') return <div className="rounded-xl bg-layer-1 px-4 py-3 text-sm text-fg-secondary">Preview unavailable: {item.name}</div>
  return <img src={src} alt={item.name} className="max-h-[85vh] w-auto rounded-xl" />
}

/** 120 §9b: a document never loads from `asset:` — a typed `blob:` of the RPC bytes, locally too. */
function OriginalDocument({ item }: { item: Item }) {
  const src = useDocumentUrl(item.path ?? '')
  if (src === null) return <div className="h-64 w-96 animate-pulse rounded-xl bg-layer-2" />
  if (src === '') return <div className="rounded-xl bg-layer-1 px-4 py-3 text-sm text-fg-secondary">Preview unavailable: {item.name}</div>
  return <iframe src={src} title={item.name} className="h-[85vh] w-[90vw] rounded-xl border-0 bg-white" />
}

/** Thumbnails (`max-h-64`) of images with a lightbox; other files as name chips (PDFs viewable). `occurrences` are the
 * stored, model-visible attachments and get a sibling corner `X` (image) or chip `X` (document) that opens the removal
 * review on exactly that occurrence; removed ones collapse to a `Removed from context` chip whose `View original` is
 * archive-only. `active` is the owning row's membership (false once folded before `tail_from`). `files` are plain paths
 * for artifact/optimistic callers and get no controls. */
export const Attachments = memo(function Attachments({
  files,
  occurrences,
  session,
  active = true,
  className,
}: {
  files?: string[]
  occurrences?: ContextAttachment[]
  session?: string
  active?: boolean
  className?: string
}) {
  const [open, setOpen] = useState<Item | null>(null)
  const list = items(files, occurrences)
  if (list.length === 0) return null
  const control = (item: Item, corner: boolean) =>
    session && item.occurrence && !item.occurrence.removed ? (
      <RemoveButton
        session={session}
        target={item.occurrence.target}
        active={active}
        icon={X}
        label={`Remove ${item.image ? 'image' : 'document'} from context: ${item.name}`}
        className={
          corner
            ? 'absolute top-0 right-0 rounded-xl text-fg-primary opacity-0 transition-opacity group-hover:opacity-100 focus-visible:opacity-100 pointer-coarse:opacity-100 [&_svg]:size-6 [&_svg]:rounded-full [&_svg]:bg-layer-1/90 [&_svg]:p-1 [&_svg]:shadow-sm'
            : 'shrink-0 text-fg-tertiary'
        }
      />
    ) : null
  return (
    <div className={cn('flex flex-wrap gap-2', className)}>
      {list.map((item) =>
        !item.occurrence?.removed && item.image && item.path ? (
          <Thumb key={item.key} item={item} onOpen={() => setOpen(item)} control={control(item, true)} chipControl={control(item, false)} />
        ) : (
          <Chip key={item.key} item={item} removed={!!item.occurrence?.removed} onOpen={() => setOpen(item)} control={control(item, false)} />
        ),
      )}
      <Dialog open={open !== null} onOpenChange={(o) => !o && setOpen(null)}>
        <DialogContent
          className="max-w-[90vw] border-0 bg-transparent p-0 shadow-none sm:max-w-[90vw]"
          showCloseButton={false}
        >
          <DialogTitle className="sr-only">{open?.occurrence?.removed ? `Archived original, removed from context: ${open.name}` : (open?.name ?? 'Attachment')}</DialogTitle>
          {open && <Original item={open} />}
          {open?.occurrence && (
            <div className="flex items-center justify-end gap-2 text-xs text-fg-secondary">
              {open.occurrence.removed ? <span className="italic">Removed from context · archive only</span> : control(open, false)}
            </div>
          )}
        </DialogContent>
      </Dialog>
    </div>
  )
})
