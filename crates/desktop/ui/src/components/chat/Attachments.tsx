import { memo, useEffect, useState } from 'react'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { resolveFile } from '@/lib/files'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'
import { useHarnesses } from '@/link/harnesses'

const IMAGE = /\.(png|jpe?g|gif|webp|bmp|svg)$/i

/** A URL for a harness path via `resolveFile` (31 §9): immediate on macOS local; elsewhere `null` until the bytes arrive, `''` on failure. */
export function useFileUrl(path: string): string | null {
  const api = useApi()
  const active = useHarnesses((s) => s.active)
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

/** `<img>` for a harness path: a dim block until the URL resolves, nothing on failure. */
export function Img({ path, className }: { path: string; className: string }) {
  const src = useFileUrl(path)
  if (src === null) return <div className={cn('h-24 w-32 animate-pulse bg-layer-2', className)} />
  if (src === '') return null
  return <img src={src} alt="" className={className} />
}

/** Thumbnails (`max-h-64`) of the image paths in `files`; click opens a lightbox. Non-images render as their file name. */
export const Attachments = memo(function Attachments({
  files,
  className,
}: {
  files: string[]
  className?: string
}) {
  const [open, setOpen] = useState<string | null>(null)
  if (files.length === 0) return null
  return (
    <div className={cn('flex flex-wrap gap-2', className)}>
      {files.map((path) =>
        IMAGE.test(path) ? (
          <button key={path} type="button" className="cursor-zoom-in" onClick={() => setOpen(path)}>
            <Img path={path} className="max-h-64 rounded-xl" />
          </button>
        ) : (
          <span key={path} className="rounded-lg bg-layer-2 px-2 py-1 text-sm text-fg-secondary">
            {path.slice(path.lastIndexOf('/') + 1)}
          </span>
        ),
      )}
      <Dialog open={open !== null} onOpenChange={(o) => !o && setOpen(null)}>
        <DialogContent
          className="max-w-[90vw] border-0 bg-transparent p-0 shadow-none sm:max-w-[90vw]"
          showCloseButton={false}
        >
          <DialogTitle className="sr-only">Image</DialogTitle>
          {open && <Img path={open} className="max-h-[90vh] w-auto rounded-xl" />}
        </DialogContent>
      </Dialog>
    </div>
  )
})
