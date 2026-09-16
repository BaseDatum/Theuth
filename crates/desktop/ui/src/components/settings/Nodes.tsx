import { useQueryClient } from '@tanstack/react-query'
import { Copy, QrCode } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Countdown, Editable } from '@/components/settings/Link'
import { Empty, Row, Section, useAction } from '@/components/settings/shared'
import { ago } from '@/components/sidebar/AgentTree'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
  AlertDialogTrigger,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { useApi } from '@/lib/rpc'
import { NODES_KEY, useNodes } from '@/link/nodes'
import type { Node } from '@/rpc/bindings/Node'
import type { NodeEvent } from '@/rpc/bindings/NodeEvent'
import type { Pairing } from '@/rpc/bindings/Pairing'
import { useConnection } from '@/store/connection'
import { on } from '@/store/sessions'

/** Every nodes bus event; any of them means the list is stale. */
const NODE_EVENTS = ['NodeAdded', 'NodeOnline', 'NodeRemoved', 'NodeRevoked'] as const satisfies readonly NodeEvent['type'][]

/** 32 §8.3 parent side: the harness's nodes with invite QR, rename, remove, and "Add node". */
export function Nodes() {
  const api = useApi()
  const qc = useQueryClient()
  const harnessName = useConnection((s) => s.info?.name)
  const nodes = useNodes()
  const keys = [NODES_KEY]
  const [key, setKey] = useState('')

  const add = useAction((k: string) => api.add_node(k, null), {
    keys,
    onSuccess: (n) => {
      toast.success(`Added ${n.name}`)
      setKey('')
    },
  })
  const rename = useAction((v: { id: string; name: string }) => api.rename_node(v.id, v.name), { keys })
  const remove = useAction((id: string) => api.remove_node(id), { keys })

  useEffect(() => {
    const invalidate = () => void qc.invalidateQueries({ queryKey: NODES_KEY })
    const offs = NODE_EVENTS.map((t) => on(t, invalidate))
    return () => {
      for (const off of offs) off()
    }
  }, [qc])

  const list = nodes.data
  return (
    <>
      <Section title="Nodes">
        <p className="pb-2 text-[13px] text-fg-secondary">Machines this harness and your devices can run sessions on.</p>
        {list?.length === 0 && <Empty>No node yet. Run `wth --install-service` on the machine and paste the key below.</Empty>}
        {list?.map((n) => (
          <NodeRow key={n.id} node={n} onRename={(name) => rename.mutate({ id: n.id, name })} onRemove={() => remove.mutate(n.id)} />
        ))}
      </Section>

      <Section title="Add node">
        <div className="flex items-center gap-2 py-2">
          <Input
            value={key}
            onChange={(e) => setKey(e.target.value)}
            onKeyDown={(e) => {
              if (e.key === 'Enter' && key.trim()) add.mutate(key.trim())
            }}
            placeholder="paste the key printed by wth --install-service"
            autoCapitalize="off"
            autoCorrect="off"
            spellCheck={false}
            className="h-9 flex-1 font-mono text-xs"
          />
          <Button variant="outline" size="sm" disabled={key.trim() === '' || add.isPending} onClick={() => add.mutate(key.trim())}>
            Add
          </Button>
        </div>
      </Section>
      <p className="text-[13px] text-fg-tertiary">
        A node trusts {harnessName ?? 'this harness'} as a device: anything this harness can do, it can do there.
      </p>
    </>
  )
}

function Dot({ on }: { on: boolean }) {
  return <span className={`inline-block size-2 shrink-0 rounded-full ${on ? 'bg-emerald-500' : 'bg-fg-tertiary/40'}`} />
}

function NodeRow({ node: n, onRename, onRemove }: { node: Node; onRename: (name: string) => void; onRemove: () => void }) {
  const api = useApi()
  const [pairing, setPairing] = useState<Pairing | null>(null)
  const invite = useAction((_: void) => api.invite_node(n.id), { onSuccess: setPairing })
  const seen = Number(n.last_seen_ms)
  const when = n.revoked
    ? 'revoked'
    : n.online
      ? `paired ${ago(new Date(Number(n.paired_at_ms)).toISOString())} ago`
      : seen
        ? `seen ${ago(new Date(seen).toISOString())} ago`
        : 'never seen'
  const parts = [n.platform, `wth ${n.version}`, n.children > 0 && `${n.children} running`, when].filter(Boolean)
  return (
    <Row
      label={
        <span className="flex items-center gap-2">
          <Dot on={n.online && !n.revoked} />
          <Editable value={n.name} width="w-48" onSave={onRename} />
        </span>
      }
      description={parts.join(' · ')}
    >
      <Popover open={pairing !== null} onOpenChange={(o) => !o && setPairing(null)}>
        <PopoverTrigger asChild>
          <Button variant="outline" size="sm" aria-label="Pair a device with this node" disabled={!n.online || n.revoked || invite.isPending} onClick={() => invite.mutate(undefined)}>
            <QrCode /> QR
          </Button>
        </PopoverTrigger>
        {pairing && (
          <PopoverContent align="end" className="flex w-auto max-w-[min(90vw,32rem)] flex-wrap items-start gap-4">
            <div className="rounded-lg bg-white">
              <QRCodeSVG value={pairing.key} size={180} marginSize={3} />
            </div>
            <div className="flex min-w-0 flex-1 flex-col gap-2 text-sm">
              <p>Scan with a phone to pair it with {n.name} directly.</p>
              <div className="flex items-center gap-2">
                <Button variant="outline" size="sm" onClick={() => void navigator.clipboard.writeText(pairing.key).then(() => toast.message('Copied'))}>
                  <Copy /> Copy key
                </Button>
                <span className="text-[13px] text-fg-secondary tabular-nums">
                  <Countdown until={Number(pairing.expires_at_ms)} />
                </span>
              </div>
              <code className="block break-all font-mono text-xs text-fg-tertiary">{pairing.key}</code>
            </div>
          </PopoverContent>
        )}
      </Popover>
      <AlertDialog>
        <AlertDialogTrigger asChild>
          <Button variant="outline" size="sm">
            Remove
          </Button>
        </AlertDialogTrigger>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Remove {n.name}?</AlertDialogTitle>
            <AlertDialogDescription>
              This harness stops running sessions there. Your devices stay paired with {n.name}; revoke them in its own Link settings if you want that too.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction onClick={onRemove}>Remove</AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Row>
  )
}
