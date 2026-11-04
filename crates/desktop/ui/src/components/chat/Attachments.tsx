import { X } from 'lucide-react'
import { memo, type ReactNode, useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogClose, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { resolveDocument, resolveFile } from '@/lib/files'
import { useApi } from '@/lib/rpc'
import { disclosureKey, useToolRuns } from '@/lib/tool-runs'
import { cn } from '@/lib/utils'
import { useEngines } from '@/link/engines'
import type { ContextAttachment } from '@/rpc/bindings/ContextAttachment'
import { RemoveButton } from './MessageAssistant'
import { useDisclosed, useScopeKey } from './disclosure'

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
export type Item = { key: string; name: string; path: string | null; image: boolean; pdf: boolean; occurrence?: ContextAttachment }

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

/** The media: the image, or the PDF in a frame (WKWebView's own viewer; iOS draws page 1 — ponytail: reuse the artifact panel's pdf.js pages if that matters).
 * Both are sized to the viewport minus room for the close button above and the toolbar below, inside the safe area; `dvh`, so iPad's URL bar never hides the bottom. */
function Original({ item }: { item: Item }) {
  return item.image ? <OriginalImage item={item} /> : <OriginalDocument item={item} />
}

function OriginalImage({ item }: { item: Item }) {
  const src = useFileUrl(item.path ?? '')
  if (src === null) return <div className="h-64 w-96 animate-pulse rounded-xl bg-layer-2" />
  if (src === '') return <div className="rounded-xl bg-layer-1 px-4 py-3 text-sm text-fg-secondary">Preview unavailable: {item.name}</div>
  return <img src={src} alt={item.name} className="max-h-[calc(100dvh_-_8rem_-_env(safe-area-inset-top)_-_env(safe-area-inset-bottom))] w-auto max-w-[calc(100vw_-_2rem_-_env(safe-area-inset-left)_-_env(safe-area-inset-right))] rounded-xl object-contain" />
}

/** 120 §9b: a document never loads from `asset:` — a typed `blob:` of the RPC bytes, locally too. */
function OriginalDocument({ item }: { item: Item }) {
  const src = useDocumentUrl(item.path ?? '')
  if (src === null) return <div className="h-64 w-96 animate-pulse rounded-xl bg-layer-2" />
  if (src === '') return <div className="rounded-xl bg-layer-1 px-4 py-3 text-sm text-fg-secondary">Preview unavailable: {item.name}</div>
  return <iframe src={src} title={item.name} className="h-[calc(100dvh_-_8rem_-_env(safe-area-inset-top)_-_env(safe-area-inset-bottom))] w-[min(90vw,1100px)] rounded-xl border-0 bg-white" />
}

/** The lightbox body, portal-free so tests can render it: a `Close` X top-right of the viewport, the media, and under it the
 * removal control as a labelled Trash2 button — never an `X`, which Sky read as "close" on iPad. The toolbar hangs below the
 * media out of flow, so the media itself sits on the viewport centre; the media's `max-h` leaves it room. */
export function Lightbox({ item, session, active }: { item: Item; session?: string; active: boolean }) {
  return (
    <>
      <DialogTitle className="sr-only">{item.occurrence?.removed ? `Archived original, removed from context: ${item.name}` : item.name}</DialogTitle>
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
      <div className="relative">
        <Original item={item} />
        {item.occurrence && (
          <div className="absolute inset-x-0 top-full mt-2 flex justify-center text-xs text-fg-secondary">
            {item.occurrence.removed ? (
              <span className="italic">Removed from context · archive only</span>
            ) : (
              session && <RemoveButton session={session} target={item.occurrence.target} active={active} text className="rounded-full bg-layer-1/80 text-fg-primary hover:bg-layer-1" />
            )}
          </div>
        )}
      </div>
    </>
  )
}

/** Thumbnails (`max-h-64`) of images with a lightbox (`Lightbox`); other files as name chips (PDFs viewable). `occurrences` are
 * the stored, model-visible attachments and get a sibling corner `X` (image) or chip `X` (document) that opens the removal
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
  const list = items(files, occurrences)
  // 171F B13: the item in the lightbox is a disclosure per item, so a remount of the card does not close it.
  const lightbox = useScopeKey('lightbox')
  const shown = useDisclosed((d) => list.find((i) => d[disclosureKey(lightbox, i.key)])?.key ?? '')
  const open = list.find((i) => i.key === shown) ?? null
  const setOpen = (item: Item | null) => {
    const { setDisclosed } = useToolRuns.getState()
    if (open) setDisclosed(disclosureKey(lightbox, open.key), false)
    if (item) setDisclosed(disclosureKey(lightbox, item.key), true)
  }
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
        {/* A full-viewport transparent flex box that centres the media (tailwind-merge drops the default centred-card classes);
            a tap that lands on the box itself — anywhere off the media and its toolbar — closes, as Esc and the X do. */}
        <DialogContent
          className="inset-0 flex h-dvh w-screen max-w-none translate-x-0 translate-y-0 items-center justify-center gap-0 rounded-none border-0 bg-transparent p-0 shadow-none sm:max-w-none"
          showCloseButton={false}
          onClick={(e) => e.target === e.currentTarget && setOpen(null)}
        >
          {open && <Lightbox item={open} session={session} active={active} />}
        </DialogContent>
      </Dialog>
    </div>
  )
})
