// 218 "Add to whiteboard…": from a chat card's menu, pick a whiteboard (or make one) and the card lands on it, right of
// what is already there. A notice confirms with Open.

import { useNavigate } from '@tanstack/react-router'
import { Plus, Presentation } from 'lucide-react'
import { useState } from 'react'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { notify } from '@/lib/notify'
import { useApi } from '@/lib/rpc'
import { useConnection } from '@/store/connection'
import { useScope } from '@/store/engine-scope'
import { type PendingCard, placeOnWhiteboard } from './place-elsewhere'
import { useCreateWhiteboard, useWhiteboards } from './useWhiteboards'

export function AddToWhiteboard({ open, onOpenChange, card, name }: { open: boolean; onOpenChange(open: boolean): void; card: () => Promise<PendingCard | null>; name: string }) {
  const api = useApi()
  const client = useConnection((s) => s.client)
  const engine = useScope().id
  const navigate = useNavigate()
  const list = useWhiteboards(50)
  const create = useCreateWhiteboard()
  const [busy, setBusy] = useState<string | null>(null)

  const add = async (doc: string | null, title: string) => {
    if (!client || busy) return
    setBusy(doc ?? 'new')
    try {
      const c = await card()
      if (!c) throw new Error('A whiteboard cannot show this kind of artifact')
      const target = doc ?? (await create(title))
      await placeOnWhiteboard({ client, api, engine, doc: target, card: c })
      onOpenChange(false)
      notify.success(`Added “${c.title}” to ${title}`, {
        action: { label: 'Open', onClick: () => void navigate({ to: '/whiteboards/$id', params: { id: target } }) },
      })
    } catch (e) {
      notify.error(e instanceof Error ? e.message : String(e))
    } finally {
      setBusy(null)
    }
  }

  const rows = list.data ?? []
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-sm">
        <DialogHeader>
          <DialogTitle>Add to whiteboard</DialogTitle>
          <DialogDescription>“{name}” becomes a card on the whiteboard you pick.</DialogDescription>
        </DialogHeader>
        <ul className="flex max-h-80 flex-col overflow-y-auto" aria-label="Whiteboards">
          <li>
            <button type="button" disabled={!!busy} onClick={() => void add(null, name.slice(0, 80) || 'Whiteboard')} className="flex min-h-11 w-full items-center gap-2 rounded-lg px-3 text-left text-sm hover:bg-layer-2 disabled:opacity-50">
              <Plus className="size-4" /> {busy === 'new' ? 'Making a whiteboard…' : 'New whiteboard'}
            </button>
          </li>
          {rows.map((w) => (
            <li key={w.id}>
              <button type="button" disabled={!!busy} onClick={() => void add(w.id, w.title || 'Untitled whiteboard')} className="flex min-h-11 w-full items-center gap-2 rounded-lg px-3 text-left text-sm hover:bg-layer-2 disabled:opacity-50">
                <Presentation className="size-4 text-fg-tertiary" />
                <span className="min-w-0 flex-1 truncate">{w.title || 'Untitled whiteboard'}</span>
                {busy === w.id && <span className="text-xs text-fg-tertiary">Adding…</span>}
              </button>
            </li>
          ))}
          {list.isSuccess && rows.length === 0 && <li className="px-3 py-2 text-sm text-fg-tertiary">No whiteboards yet.</li>}
        </ul>
      </DialogContent>
    </Dialog>
  )
}
