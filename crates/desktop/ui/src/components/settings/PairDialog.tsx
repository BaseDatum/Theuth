import { useQuery, useQueryClient } from '@tanstack/react-query'
import { CircleCheck, TriangleAlert } from 'lucide-react'
import { type ReactNode, type Ref, useCallback, useEffect, useRef, useState } from 'react'
import { errorText } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { putPair, queryKeys, useApi } from '@/lib/rpc'
import type { NearbyEngine } from '@/rpc/bindings/NearbyEngine'
import type { PairCeremony } from '@/rpc/bindings/PairCeremony'

/** What the pair dialog shows: `starting` until `mesh_pair_nearby` answers; `code` until this side presses Pair, then
 * `waiting` for the other side; `paired` / `rejected` / `expired` end it; `failed` is an RPC refusal. */
export type PairView = 'starting' | 'failed' | 'code' | 'waiting' | 'paired' | 'rejected' | 'expired'

export function pairView(c: PairCeremony | undefined, error: string | null, answered: boolean): PairView {
  if (error) return 'failed'
  if (!c) return 'starting'
  if (c.state === 'paired' || c.state === 'rejected' || c.state === 'expired') return c.state
  return answered ? 'waiting' : 'code'
}

/** `482913` → `482 913`; the engine's `482 913` as is. */
export const spaced = (code: string): string => code.replace(/^(\d{3})(\d{3})$/, '$1 $2')

const WARNING = 'pair-vault-warning'

/** The vault-sharing warning (§6.8); `id` is what the dialog's `aria-describedby` names. */
export function VaultWarning({ id }: { id: string }) {
  return (
    <div className="flex items-start gap-2 rounded-lg border border-warning-border/40 bg-warning-bg px-3 py-2 text-[13px] text-warning">
      <TriangleAlert className="mt-0.5 size-4 shrink-0" aria-hidden="true" />
      <p id={id}>Pairing shares your secrets vault, sessions and tasks — only pair your own machines.</p>
    </div>
  )
}

/** One ceremony, either side: the code, the check line, the vault warning, Pair / Cancel, and how it ended (§6.8, mock panel 2). */
export function PairBody({
  name,
  detail,
  ceremony,
  view,
  error,
  onPair,
  onCancel,
  cancelRef,
}: {
  name: string
  detail: string
  ceremony: PairCeremony | undefined
  view: PairView
  error: string | null
  onPair: () => void
  onCancel: () => void
  /** The button that takes the dialog's initial focus: Cancel (or Done), never Pair. */
  cancelRef?: Ref<HTMLButtonElement>
}) {
  const close = (label = 'Close') => (
    <Button ref={cancelRef} variant="outline" onClick={onCancel}>
      {label}
    </Button>
  )
  const end: Partial<Record<PairView, [ReactNode, ReactNode]>> = {
    starting: [<p className="text-sm text-fg-secondary">Connecting to {name}…</p>, close('Cancel')],
    failed: [<p className="text-sm text-danger">{error}</p>, close()],
    paired: [
      <p className="flex items-center gap-2 text-sm">
        <CircleCheck className="size-4 text-success" aria-hidden="true" />
        Paired with {name}. It now shares this mesh's vault, sessions and tasks.
      </p>,
      <Button ref={cancelRef} onClick={onCancel}>Done</Button>,
    ],
    rejected: [<p className="text-sm text-danger">Not paired: {name} pressed Cancel, or the codes did not match.</p>, close()],
    expired: [<p className="text-sm text-fg-secondary">The pairing window closed before both machines pressed Pair. Start again from Nearby.</p>, close()],
  }
  const ended = end[view]
  return (
    <div className="flex flex-col gap-5">
      <div>
        <DialogTitle className="text-lg font-semibold">Pair with {name}</DialogTitle>
        {detail && <p className="text-[13px] text-fg-tertiary">{detail}</p>}
      </div>
      {ended ? (
        <>
          {ended[0]}
          <div className="flex justify-end gap-2">{ended[1]}</div>
        </>
      ) : (
        <>
          <div className="rounded-xl bg-layer-2 px-4 py-5 text-center">
            <div className="font-mono text-4xl font-semibold tracking-[0.15em] tabular-nums" role="group" aria-label="Pairing code">
              {spaced(ceremony?.code ?? '')}
            </div>
            <p className="mt-2 text-sm text-fg-secondary">Check that {name} shows the same code</p>
          </div>
          <VaultWarning id={WARNING} />
          <div className="flex items-center justify-end gap-2">
            {view === 'waiting' && <span className="mr-auto text-[13px] text-fg-secondary">Waiting for {name} to press Pair…</span>}
            <Button onClick={onPair} disabled={view === 'waiting'}>
              Pair
            </Button>
            {close('Cancel')}
          </div>
        </>
      )}
    </div>
  )
}

type Flow = { id: string | null; name: string; detail: string; answered: boolean; error: string | null }

/**
 * 183 §6.8: Nearby › Pair… (`start`) dials and shows the code; a stranger's request while this page is open (a
 * `MeshEvent.pair` in `waiting` this side did not start) opens the same dialog. Pair / Cancel answer with
 * `mesh_pair_answer`; closing an open ceremony cancels it.
 */
export function usePairFlow(): { start: (n: NearbyEngine) => void; dialog: ReactNode } {
  const api = useApi()
  const qc = useQueryClient()
  const ceremonies = useQuery<Record<string, PairCeremony>>({ queryKey: queryKeys.meshPair, queryFn: () => ({}), enabled: false, staleTime: Infinity }).data
  const [flow, setFlow] = useState<Flow | null>(null)
  const [seen, setSeen] = useState<readonly string[]>([])
  const cancelRef = useRef<HTMLButtonElement | null>(null)
  // Cancel (or Done) takes the focus each time it mounts: `starting` → `code` swaps the button, and Radix would
  // otherwise park the focus on the dialog itself. A button the user already focused in this dialog keeps it.
  const cancelMount = useCallback((b: HTMLButtonElement | null) => {
    cancelRef.current = b
    const f = document.activeElement
    if (b && !(f instanceof HTMLButtonElement && b.closest('[role=dialog]')?.contains(f))) b.focus()
  }, [])
  // 183 R5: each dial has a token; Cancel or a newer dial makes it stale, and a stale reply's ceremony is refused,
  // never shown (a hanging dial cancelled earlier cannot land on the next dialog). `dialing` counts pending dials:
  // the engine's event can beat the reply, and an unknown waiting ceremony must not open meanwhile.
  const dial = useRef(0)
  const dialing = useRef(0)

  useEffect(() => {
    const c = dialing.current ? undefined : Object.values(ceremonies ?? {}).find((c) => c.state === 'waiting' && c.expires_ms > Date.now() && !seen.includes(c.id))
    if (!flow && c) setFlow({ id: c.id, name: c.name, detail: [c.os_name, c.addr, c.peer.slice(0, 8)].filter(Boolean).join(' · '), answered: false, error: null })
  }, [flow, ceremonies, seen])

  const start = (n: NearbyEngine) => {
    const token = ++dial.current
    dialing.current++
    setFlow({ id: null, name: n.name, detail: [n.os_name, n.addr].filter(Boolean).join(' · '), answered: false, error: null })
    api.mesh_pair_nearby(n.id).then(
      (c) => {
        dialing.current--
        putPair(qc, c)
        if (token !== dial.current) {
          setSeen((s) => [...s, c.id])
          void api.mesh_pair_answer(c.id, false).catch(() => undefined)
        } else setFlow((f) => f && { ...f, id: c.id })
      },
      (e: unknown) => {
        dialing.current--
        if (token === dial.current) setFlow((f) => f && { ...f, error: errorText(e) })
      },
    )
  }
  if (!flow) return { start, dialog: null }

  const ceremony = flow.id === null ? undefined : ceremonies?.[flow.id]
  const view = pairView(ceremony, flow.error, flow.answered)
  const close = () => {
    if (flow.id === null) dial.current++
    else {
      if (view === 'starting' || view === 'code' || view === 'waiting') void api.mesh_pair_answer(flow.id, false).catch(() => undefined)
      setSeen((s) => [...s, flow.id!])
    }
    setFlow(null)
  }
  const pair = () => {
    if (flow.id === null) return
    setFlow({ ...flow, answered: true })
    api.mesh_pair_answer(flow.id, true).catch((e: unknown) => setFlow((f) => f && { ...f, error: errorText(e) }))
  }
  const dialog = (
    <Dialog open onOpenChange={(o) => !o && close()}>
      <DialogContent
        className="sm:max-w-md"
        aria-describedby={view === 'code' || view === 'waiting' ? WARNING : undefined}
        onOpenAutoFocus={(e) => {
          e.preventDefault()
          cancelRef.current?.focus()
        }}
      >
        <PairBody name={flow.name} detail={flow.detail} ceremony={ceremony} view={view} error={flow.error} onPair={pair} onCancel={close} cancelRef={cancelMount} />
      </DialogContent>
    </Dialog>
  )
  return { start, dialog }
}
