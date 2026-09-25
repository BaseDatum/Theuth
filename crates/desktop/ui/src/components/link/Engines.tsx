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
import { type Engine, LOCAL, LOCAL_NAME, type Presence, useEngines } from '@/link/engines'
import { Dot, meshChip, useMembersLine } from './EngineSwitcher'

/** Local label; Enter/blur saves, Escape restores. */
function Name({ engine }: { engine: Engine }) {
  const rename = useEngines((s) => s.rename)
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState(engine.name)
  const save = () => {
    const name = draft.trim()
    if (name && name !== engine.name) rename(engine.id, name)
    else setDraft(engine.name)
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
            setDraft(engine.name)
            setEditing(false)
          }
        }}
        className="h-7 w-48"
      />
    )
  }
  return (
    <button type="button" className="group flex items-center gap-1.5 text-left" onClick={() => setEditing(true)} aria-label="Rename">
      <span className="truncate">{engine.name}</span>
      <Pencil className="size-3 text-fg-tertiary opacity-0 group-hover:opacity-100 pointer-coarse:opacity-100" />
    </button>
  )
}

/**
 * Settings › Engines (31 §7.5 "Manage"): paired engines with presence, local
 * rename, reorder and Forget. Forgetting the active one falls back to the local
 * daemon or the first remaining; with none left `App` shows the pair screen.
 * A mesh row (69 §4) shows its mesh name, `n of m engines online` and the failover chip.
 */
export function Engines() {
  const navigate = useNavigate()
  const engines = useEngines((s) => s.engines)
  const presence = useEngines((s) => s.presence)
  const { remove, reorder } = useEngines.getState()
  const [forget, setForget] = useState<Engine | null>(null)

  const move = (i: number, by: -1 | 1) => {
    const ids = engines.map((h) => h.id)
    const [id] = ids.splice(i, 1)
    ids.splice(i + by, 0, id!)
    reorder(ids)
  }

  return (
    <>
      <Section
        title="Engines"
        actions={
          <Button variant="outline" size="sm" onClick={() => void navigate({ to: '/link' })}>
            <Plus /> Add engine…
          </Button>
        }
      >
        {engines.length === 0 && <Empty>No engines paired.</Empty>}
        {engines.map((h, i) => {
          const p = presence[h.id] ?? 'offline'
          const via = h.parent && (h.parent === LOCAL ? LOCAL_NAME : engines.find((x) => x.id === h.parent)?.name)
          const paired = h.pairedAt ? `paired ${ago(new Date(h.pairedAt).toISOString())} ago` : 'not paired yet'
          return (
            <Row
              key={h.id}
              label={
                <span className="flex items-center gap-2">
                  <Dot presence={p} />
                  <Name engine={h} />
                  {via && <span className="text-[13px] text-fg-tertiary">via {via}</span>}
                  {h.kind === 'mesh' && <span className="text-[13px] text-fg-tertiary">mesh{h.meshName && h.meshName !== h.name ? ` · ${h.meshName}` : ''}</span>}
                </span>
              }
              description={h.kind === 'mesh' ? <MeshLine engine={h} presence={p} paired={paired} /> : `${h.relayHost} · ${paired} · ${p}`}
            >
              <Button variant="ghost" size="icon-sm" aria-label="Move up" disabled={i === 0} onClick={() => move(i, -1)}>
                <ArrowUp />
              </Button>
              <Button variant="ghost" size="icon-sm" aria-label="Move down" disabled={i === engines.length - 1} onClick={() => move(i, 1)}>
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

/** `relay · paired … · 2 of 3 engines online · relay connected` — or the failover chip instead of the last two. */
function MeshLine({ engine: h, presence, paired }: { engine: Engine; presence: Presence; paired: string }) {
  const members = useMembersLine(h.id)
  const chip = meshChip(presence)
  return <>{[h.relayHost, paired, chip ?? members, chip ? null : 'relay connected'].filter(Boolean).join(' · ')}</>
}
