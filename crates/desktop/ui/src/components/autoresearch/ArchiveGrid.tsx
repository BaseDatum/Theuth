import type { KindView } from '@/rpc/bindings/KindView'

/** 159 W4: one grid per island, cells coloured by score, the champion starred. Owner: W4; the skeleton renders a placeholder until the wave lands. */
export function ArchiveGrid({ view }: { view: Extract<KindView, { kind: 'archive' }> }) {
  return (
    <div className="rounded-md border border-dashed p-4 text-sm text-fg-tertiary" data-kind={view.kind}>
      This kind needs a newer app (W4 is not built yet).
    </div>
  )
}
