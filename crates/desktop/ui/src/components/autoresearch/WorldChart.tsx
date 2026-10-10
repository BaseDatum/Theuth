import type { KindView } from '@/rpc/bindings/KindView'

/** 159 W6: both arms per window, Λ against the 1/α line, units and exposure. Owner: W6; the skeleton renders a placeholder until the wave lands. */
export function WorldChart({ view }: { view: Extract<KindView, { kind: 'world' }> }) {
  return (
    <div className="rounded-md border border-dashed p-4 text-sm text-fg-tertiary" data-kind={view.kind}>
      This kind needs a newer app (W6 is not built yet).
    </div>
  )
}
