// 156 D13: "Enroll as new speaker…" for the chosen segments. Name (required) and an optional knowledge entity,
// as the legend chip's enroll. The engine's "a speaker named X is already enrolled" refusal becomes an inline
// [Add to X instead] that folds the same speech into that profile. Other errors toast as the engine wrote them.

import { useQueryClient } from '@tanstack/react-query'
import { LoaderCircle } from 'lucide-react'
import { useState } from 'react'
import { toast } from 'sonner'
import { errorText } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { queryKeys } from '@/lib/rpc'
import { alreadyEnrolled, secondsText } from '@/lib/segment-select'
import type { EnrollTarget } from '@/rpc/bindings/EnrollTarget'
import type { Enrolled } from '@/rpc/bindings/Enrolled'
import type { Profile } from '@/rpc/bindings/Profile'

export interface EnrollDialogProps {
  open: boolean
  onOpenChange: (open: boolean) => void
  /** How many segments and how much speech the voiceprint will come from. */
  count: number
  speechMs: number
  profiles: Profile[] | undefined
  /** The raw RPC for these segments; the dialog owns the error handling. */
  enroll: (target: EnrollTarget) => Promise<Enrolled>
  onDone: (r: Enrolled, target: EnrollTarget) => void
}

export function EnrollDialog({ open, onOpenChange, count, speechMs, profiles, enroll, onDone }: EnrollDialogProps) {
  const qc = useQueryClient()
  const [name, setName] = useState('')
  const [entity, setEntity] = useState('')
  const [pending, setPending] = useState(false)
  /** The name the engine said is taken, shown with [Add to X instead]. */
  const [taken, setTaken] = useState<string | null>(null)
  const existing = taken ? profiles?.find((p) => p.name.toLowerCase() === taken.toLowerCase()) : undefined

  const run = async (target: EnrollTarget) => {
    setPending(true)
    try {
      const r = await enroll(target)
      onOpenChange(false)
      onDone(r, target)
    } catch (e) {
      const msg = errorText(e)
      const who = alreadyEnrolled(msg)
      if (who) {
        setTaken(who)
        void qc.invalidateQueries({ queryKey: queryKeys.meetingSpeakers })
      } else toast.error(msg)
    } finally {
      setPending(false)
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(o) => {
        if (!o) setTaken(null)
        onOpenChange(o)
      }}
    >
      <DialogContent className="sm:max-w-sm">
        <form
          className="flex flex-col gap-4"
          onSubmit={(e) => {
            e.preventDefault()
            if (name.trim()) void run({ kind: 'New', name: name.trim(), entity: entity.trim() || null })
          }}
        >
          <DialogHeader>
            <DialogTitle className="text-base">Enroll a new speaker</DialogTitle>
            <DialogDescription>
              A voiceprint from {count === 1 ? 'this segment' : `${count} segments`} ({secondsText(speechMs)} of speech).
            </DialogDescription>
          </DialogHeader>
          <Input
            autoFocus
            placeholder="Name"
            aria-label="Name"
            value={name}
            onChange={(e) => {
              setName(e.target.value)
              setTaken(null)
            }}
          />
          <Input placeholder="Knowledge entity (optional)" aria-label="Knowledge entity" value={entity} onChange={(e) => setEntity(e.target.value)} />
          {taken && (
            <div className="flex items-center justify-between gap-2 rounded-md border border-border-subtle bg-accent/40 px-3 py-2 text-[13px]">
              <span>{taken} is already enrolled.</span>
              {existing && (
                <Button size="xs" variant="outline" type="button" disabled={pending} onClick={() => void run({ kind: 'Existing', profile: existing.id })}>
                  Add to {existing.name} instead
                </Button>
              )}
            </div>
          )}
          <DialogFooter>
            <Button type="button" variant="ghost" onClick={() => onOpenChange(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={!name.trim() || pending}>
              {pending && <LoaderCircle className="animate-spin" />} Enroll
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
