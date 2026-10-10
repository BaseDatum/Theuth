/** 159 W3: both outputs side by side, the two verdicts with their reasons, the human's answer when there is one.
 * Owner: W3; the skeleton renders a placeholder. Pairs come from `autoresearch.judgments`. */
export function PairViewer({ slug, n }: { slug: string; n: number | null }) {
  return (
    <div className="rounded-md border border-dashed p-4 text-sm text-fg-tertiary" data-slug={slug} data-n={n ?? ''}>
      Blind pairs need a newer app (W3 is not built yet).
    </div>
  )
}
