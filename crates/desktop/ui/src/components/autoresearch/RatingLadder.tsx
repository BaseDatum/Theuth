import type { KindView } from '@/rpc/bindings/KindView'

/** 159 W3: the Bradley–Terry ladder of kept versions (θ ± 2 se) and the agreement figure. Owner: W3; the skeleton renders a placeholder until the wave lands. */
export function RatingLadder({ view }: { view: Extract<KindView, { kind: 'ratings' }> }) {
  return (
    <div className="rounded-md border border-dashed p-4 text-sm text-fg-tertiary" data-kind={view.kind}>
      This kind needs a newer app (W3 is not built yet).
    </div>
  )
}
