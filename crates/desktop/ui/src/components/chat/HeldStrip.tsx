import { FileText, X } from 'lucide-react'
import { useState } from 'react'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Textarea } from '@/components/ui/textarea'
import { drop, label, type Paste, type Pastes, retext } from '@/lib/pastes'
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

/** A pasted block: icon, size and the first line; click opens it for review or trimming. */
function TextCard({ paste, onChange }: { paste: Extract<Paste, { kind: 'text' }>; onChange(text: string): void }) {
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
          <span className="ml-auto font-normal text-fg-tertiary">{label(paste)}</span>
        </span>
        <span className="line-clamp-2 font-mono text-fg-secondary wrap-anywhere">{first}</span>
      </button>
      <Dialog open={open} onOpenChange={setOpen}>
        <DialogContent className="flex max-h-[85vh] flex-col sm:max-w-2xl">
          <DialogHeader>
            <DialogTitle>Pasted text</DialogTitle>
            <DialogDescription>{label({ kind: 'text', text: value })} · sent after your message</DialogDescription>
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

/** What the next send carries besides the typed words: pasted blocks as cards, images as thumbnails, PDFs as name chips;
 * each with a corner `X`. Nothing when nothing is held. */
export function HeldStrip({ pastes, onChange }: { pastes: Pastes; onChange(next: Pastes): void }) {
  if (pastes.size === 0) return null
  return (
    <div className="flex flex-wrap gap-3 px-2 pt-2 pb-1">
      {[...pastes].map(([i, p]) => (
        <div key={i} className="relative">
          {p.kind === 'text' ? (
            <TextCard paste={p} onChange={(text) => onChange(retext(i, text, pastes))} />
          ) : (
            <Attachments files={[p.path]} className="[&_img]:max-h-20" />
          )}
          <Remove what={p.kind === 'text' ? 'pasted text' : 'attachment'} onClick={() => onChange(drop(i, pastes))} />
        </div>
      ))}
    </div>
  )
}
