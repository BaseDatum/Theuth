import { AlertCircle, FileText, LoaderCircle, RotateCw, X } from 'lucide-react'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Textarea } from '@/components/ui/textarea'
import type { AttachRef } from '@/lib/attach'
import { label, type Paste, type Pastes, retext, token } from '@/lib/pastes'
import { cn } from '@/lib/utils'
import { AttachChip } from './AttachChip'
import { Attachments } from './Attachments'
import type { Upload } from './uploads'

/** Corner `X` shared by every held item. */
function Remove({ onClick, what }: { onClick(): void; what: string }) {
  return (
    <button
      type="button"
      aria-label={`Remove ${what}`}
      onClick={onClick}
      className="absolute -top-1.5 -right-1.5 flex size-5 items-center justify-center rounded-full bg-foreground text-background"
    >
      <X className="size-3" />
    </button>
  )
}

/** Pasted block `n`: icon, its number (matching the `⟦paste n⟧` token in the box), size and the first line; click opens it for review or trimming. */
function TextCard({ n, paste, onChange }: { n: number; paste: Extract<Paste, { kind: 'text' }>; onChange(text: string): void }) {
  const [open, setOpen] = useState(false)
  const [value, setValue] = useState(paste.text)
  const first = paste.text.trimStart().split('\n', 1)[0] ?? ''
  return (
    <>
      <button
        type="button"
        onClick={() => {
          setValue(paste.text)
          setOpen(true)
        }}
        className="flex h-20 w-44 flex-col gap-1 rounded-xl border border-border-subtle bg-layer-1 px-3 py-2 text-left text-xs hover:bg-layer-2"
      >
        <span className="flex items-center gap-1.5 font-medium text-foreground">
          <FileText className="size-3.5 shrink-0 text-fg-secondary" /> Pasted text
          <span className="ml-auto font-normal text-fg-tertiary">
            #{n} · {label(paste)}
          </span>
        </span>
        <span className="line-clamp-2 font-mono text-fg-secondary wrap-anywhere">{first}</span>
      </button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="flex max-h-[85vh] flex-col sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>Pasted text #{n}</DialogTitle>
            <DialogDescription>
              {label({ kind: 'text', text: value })} · sent where {token(n)} stands in your message
            </DialogDescription>
          </DialogHeader>
          <Textarea
            value={value}
            onChange={(e) => setValue(e.target.value)}
            spellCheck={false}
            className="min-h-0 flex-1 resize-none font-mono text-xs leading-5 whitespace-pre"
          />
          <DialogFooter>
            <Button variant="ghost" onClick={() => setOpen(false)}>
              Cancel
            </Button>
            <Button
              disabled={!value.trim()}
              onClick={() => {
                onChange(value)
                setOpen(false)
              }}
            >
              Done
            </Button>
          </DialogFooter>
        </DialogContent>
      </Dialog>
    </>
  )
}

/**
 * A file on its way (`uploads.ts`), thumbnail-sized: the image itself dimmed under a spinner when it is one (a local
 * preview), else a file icon; the name under it. Indeterminate: `put_attachment` reports no bytes. The spinner exists
 * only while pending, so nothing animates once the upload is over. A failed one shows why, with retry; the corner ✕
 * cancels or removes either way.
 */
export function UploadTile({ upload, onRetry }: { upload: Upload; onRetry(): void }) {
  const failed = upload.status === 'error'
  return (
    <div
      data-upload={upload.status}
      role="status"
      aria-label={failed ? `${upload.name} failed to upload: ${upload.error ?? ''}` : `Uploading ${upload.name}`}
      title={failed ? upload.error : upload.name}
      className={cn('relative flex size-20 flex-col items-center justify-center gap-1 overflow-hidden rounded-xl border bg-layer-1 text-[10px]', failed ? 'border-danger/60' : 'border-border-subtle')}
    >
      {upload.preview && <img src={upload.preview} alt="" className="absolute inset-0 size-full object-cover opacity-40" />}
      {failed ? (
        <button
          type="button"
          aria-label={`Retry ${upload.name}`}
          onClick={onRetry}
          className="relative flex flex-col items-center gap-0.5 rounded-md px-1 text-danger outline-none hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring/50"
        >
          <AlertCircle className="size-4" />
          <span className="flex items-center gap-0.5 font-medium">
            <RotateCw className="size-3" /> Retry
          </span>
        </button>
      ) : (
        <LoaderCircle data-upload-spinner="" className="relative size-5 animate-spin text-fg-secondary" />
      )}
      <span className="relative w-full truncate px-1.5 text-center text-fg-secondary">{upload.name}</span>
    </div>
  )
}

/** What the next send carries besides the typed words: the pinned reference (59 §7.3, not removable here), pasted blocks
 * as numbered cards (their `⟦paste n⟧` tokens sit in the box), images as thumbnails, PDFs as name chips, attached references
 * as chips (66 §7.6: click for subtitle and Open); each with a corner `X` (`onRemove`, so the host can drop the token too).
 * Files still uploading follow as `UploadTile`s (✕ `onCancelUpload`, retry `onRetryUpload`). Nothing when nothing is held. */
export function HeldStrip({
  pastes,
  pinned,
  onChange,
  onRemove,
  uploads = [],
  onCancelUpload,
  onRetryUpload,
}: {
  pastes: Pastes
  pinned?: AttachRef
  onChange(next: Pastes): void
  onRemove(i: number): void
  uploads?: readonly Upload[]
  onCancelUpload?(id: string): void
  onRetryUpload?(id: string): void
}) {
  if (pastes.size === 0 && !pinned && uploads.length === 0) return null
  return (
    <div className="flex flex-wrap items-end gap-3 px-2 pt-2 pb-1">
      {pinned && <AttachChip item={pinned} />}
      {[...pastes].map(([i, p]) => (
        <div key={i} className="relative">
          {p.kind === 'text' ? (
            <TextCard n={i} paste={p} onChange={(text) => onChange(retext(i, text, pastes))} />
          ) : p.kind === 'ref' ? (
            <AttachChip item={p.ref} />
          ) : (
            <Attachments files={[p.path]} className="[&_img]:max-h-20" />
          )}
          <Remove what={p.kind === 'text' ? `pasted text #${i}` : p.kind === 'ref' ? p.ref.title : 'attachment'} onClick={() => onRemove(i)} />
        </div>
      ))}
      {uploads.map((u) => (
        <div key={u.id} className="relative">
          <UploadTile upload={u} onRetry={() => onRetryUpload?.(u.id)} />
          <Remove what={u.status === 'error' ? u.name : `upload of ${u.name}`} onClick={() => onCancelUpload?.(u.id)} />
        </div>
      ))}
    </div>
  )
}
