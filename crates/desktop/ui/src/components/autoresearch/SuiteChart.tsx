import type { KindView } from '@/rpc/bindings/KindView'

/** 159 W7: per fixture, bars for the incumbent and candidate mechanism, training and held-out apart. Owner: W7; the skeleton renders a placeholder until the wave lands. */
export function SuiteChart({ view }: { view: Extract<KindView, { kind: 'suite' }> }) {
  return (
    <div className="rounded-md border border-dashed p-4 text-sm text-fg-tertiary" data-kind={view.kind}>
      This kind needs a newer app (W7 is not built yet).
    </div>
  )
}
