import type { KindView } from '@/rpc/bindings/KindView'

/** 159 W1: the Pareto front: every experiment on two objectives, the front as a staircase. Owner: W1; the skeleton renders a placeholder until the wave lands. */
export function FrontChart({ view }: { view: Extract<KindView, { kind: 'front' }> }) {
  return (
    <div className="rounded-md border border-dashed p-4 text-sm text-fg-tertiary" data-kind={view.kind}>
      This kind needs a newer app (W1 is not built yet).
    </div>
  )
}
