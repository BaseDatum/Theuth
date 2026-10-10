import type { KindView } from '@/rpc/bindings/KindView'

/** 159 W2: the case × experiment heat strip with the val and holdout staircases. Owner: W2; the skeleton renders a placeholder until the wave lands. */
export function CaseStrip({ view }: { view: Extract<KindView, { kind: 'cases' }> }) {
  return (
    <div className="rounded-md border border-dashed p-4 text-sm text-fg-tertiary" data-kind={view.kind}>
      This kind needs a newer app (W2 is not built yet).
    </div>
  )
}
