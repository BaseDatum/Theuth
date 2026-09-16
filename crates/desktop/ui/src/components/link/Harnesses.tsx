import { useNavigate } from '@tanstack/react-router'
import { ArrowDown, ArrowUp, Pencil, Plus } from 'lucide-react'
import { useState } from 'react'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ago } from '@/components/sidebar/AgentTree'
import { Empty, Row, Section } from '@/components/settings/shared'
import { type Harness, LOCAL, LOCAL_NAME, useHarnesses } from '@/link/harnesses'
import { Dot } from './HarnessSwitcher'

/** Local label; Enter/blur saves, Escape restores. */
function Name({ harness }: { harness: Harness }) {
  const rename = useHarnesses((s) => s.rename)
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(harness.name)
  const save = () => {
    const name = draft.trim()
    if (name && name !== harness.name) rename(harness.id, name)
    else setDraft(harness.name)
    setEditing(false)
  }
  if (editing) {
    return (
      <Input
        autoFocus
        value={draft}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={save}
        onKeyDown={(e) => {
          if (e.key === 'Enter') save()
          if (e.key === 'Escape') {
            setDraft(harness.name)
            setEditing(false)
          }
        }}
        className="h-7 w-48"
      />
    )
  }
  return (
    <button type="button" className="group flex items-center gap-1.5 text-left" onClick={() => setEditing(true)} aria-label="Rename">
      <span className="truncate">{harness.name}</span>
      <Pencil className="size-3 text-fg-tertiary opacity-0 group-hover:opacity-100 pointer-coarse:opacity-100" />
    </button>
  )
}

/**
 * Settings › Harnesses (31 §7.5 "Manage"): paired harnesses with presence, local
 * rename, reorder and Forget. Forgetting the active one falls back to the local
 * daemon or the first remaining; with none left `App` shows the pair screen.
 */
export function Harnesses() {
  const navigate = useNavigate()
  const harnesses = useHarnesses((s) => s.harnesses)
  const presence = useHarnesses((s) => s.presence)
  const { remove, reorder } = useHarnesses.getState()
  const [forget, setForget] = useState<Harness | null>(null)

  const move = (i: number, by: -1 | 1) => {
    const ids = harnesses.map((h) => h.id)
    const [id] = ids.splice(i, 1)
    ids.splice(i + by, 0, id!)
    reorder(ids)
  }

  return (
    <>
      <Section
        title="Harnesses"
        actions={
          <Button variant="outline" size="sm" onClick={() => void navigate({ to: '/link' })}>
            <Plus /> Add harness…
          </Button>
        }
      >
        {harnesses.length === 0 && <Empty>No harnesses paired.</Empty>}
        {harnesses.map((h, i) => {
          const p = presence[h.id] ?? 'offline'
          const via = h.parent && (h.parent === LOCAL ? LOCAL_NAME : harnesses.find((x) => x.id === h.parent)?.name)
          return (
            <Row
              key={h.id}
              label={
                <span className="flex items-center gap-2">
                  <Dot presence={p} />
                  <Name harness={h} />
                  {via && <span className="text-[13px] text-fg-tertiary">via {via}</span>}
                </span>
              }
              description={`${h.relayHost} · ${h.pairedAt ? `paired ${ago(new Date(h.pairedAt).toISOString())} ago` : 'not paired yet'} · ${p}`}
            >
              <Button variant="ghost" size="icon-sm" aria-label="Move up" disabled={i === 0} onClick={() => move(i, -1)}>
                <ArrowUp />
              </Button>
              <Button variant="ghost" size="icon-sm" aria-label="Move down" disabled={i === harnesses.length - 1} onClick={() => move(i, 1)}>
                <ArrowDown />
              </Button>
              <Button variant="outline" size="sm" onClick={() => setForget(h)}>
                Forget
              </Button>
            </Row>
          )
        })}
      </Section>
      <AlertDialog open={forget !== null} onOpenChange={(o) => !o && setForget(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Forget {forget?.name}?</AlertDialogTitle>
            <AlertDialogDescription>
              This device stays listed on {forget?.name} until you revoke it there.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction
              onClick={() => {
                if (forget) remove(forget.id)
                setForget(null)
              }}
            >
              Forget
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </>
  )
}
