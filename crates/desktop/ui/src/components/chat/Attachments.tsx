import { convertFileSrc } from '@tauri-apps/api/core'
import { memo, useState } from 'react'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { isTauri } from '@/lib/tauri'
import { cn } from '@/lib/utils'

const IMAGE = /\.(png|jpe?g|gif|webp|bmp|svg)$/i

/** `asset://` URL for a path under `<data_dir>/attachments`; the bare path in a plain browser (dev). */
const src = (path: string) => (isTauri() ? convertFileSrc(path) : path)

/** Thumbnails (`max-h-64`) of the image paths in `files`; click opens a lightbox. Non-images render as their file name. */
export const Attachments = memo(function Attachments({ files, className }: { files: string[]; className?: string }) {
  const [open, setOpen] = useState<string | null>(null)
  if (files.length === 0) return null
  return (
    <div className={cn('flex flex-wrap gap-2', className)}>
      {files.map((path) =>
        IMAGE.test(path) ? (
          <button key={path} type="button" className="cursor-zoom-in" onClick={() => setOpen(path)}>
            <img src={src(path)} alt="" className="max-h-64 rounded-xl" />
          </button>
        ) : (
          <span key={path} className="rounded-lg bg-layer-2 px-2 py-1 text-sm text-fg-secondary">
            {path.slice(path.lastIndexOf('/') + 1)}
          </span>
        ),
      )}
      <Dialog open={open !== null} onOpenChange={(o) => !o && setOpen(null)}>
        <DialogContent className="max-w-[90vw] border-0 bg-transparent p-0 shadow-none sm:max-w-[90vw]" showCloseButton={false}>
          <DialogTitle className="sr-only">Image</DialogTitle>
          {open && <img src={src(open)} alt="" className="max-h-[90vh] w-auto rounded-xl" />}
        </DialogContent>
      </Dialog>
    </div>
  )
})
