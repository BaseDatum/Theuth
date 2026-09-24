import { FileText, X } from 'lucide-react'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Textarea } from '@/components/ui/textarea'
import type { AttachRef } from '@/lib/attach'
import { label, type Paste, type Pastes, retext, token } from '@/lib/pastes'
import { AttachChip } from './AttachChip'
import { Attachments } from './Attachments'

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

/** What the next send carries besides the typed words: the pinned reference (59 §7.3, not removable here), pasted blocks
 * as numbered cards (their `⟦paste n⟧` tokens sit in the box), images as thumbnails, PDFs as name chips, attached references
 * as chips (66 §7.6: click for subtitle and Open); each with a corner `X` (`onRemove`, so the host can drop the token too). Nothing when nothing is held. */
export function HeldStrip({
  pastes,
  pinned,
  onChange,
  onRemove,
}: {
  pastes: Pastes
  pinned?: AttachRef
  onChange(next: Pastes): void
  onRemove(i: number): void
}) {
  if (pastes.size === 0 && !pinned) return null
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
    </div>
  )
}
