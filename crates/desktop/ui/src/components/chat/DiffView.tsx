import { useMemo } from 'react'
import { DiffSections } from '@/components/code/DiffBody'
import { type DiffModel, type DiffPair, pairsDiff, parseUnified } from '@/lib/editDiff'
import { disclosureKey, useDisclosure } from '@/lib/tool-runs'

const ROW_CAP = 400

export function DiffStats({ added, removed }: Pick<DiffModel, 'added' | 'removed'>) {
  return <span className="inline-flex shrink-0 gap-2 font-mono text-xs" aria-label={`${added} added, ${removed} removed`}>
    <span className="text-success">+{added}</span><span className="text-danger">−{removed}</span>
  </span>
}

/** Unified patches and argument-only snippets share the same file/hunk/line layout. `disclosure` is the owning
 * card's key: Show all lives in the disclosure store, so a remount or reload keeps it. */
export function DiffView({ model, diff, pairs, disclosure }: { model?: DiffModel; diff?: string; pairs?: DiffPair[]; disclosure: string }) {
  const parsed = useMemo(() => model ?? (diff !== undefined ? parseUnified(diff) : pairsDiff(pairs ?? [])), [model, diff, pairs])
  const [all, setAll] = useDisclosure(disclosureKey(disclosure, 'diff-all'))
  return <DiffSections
    files={parsed.files}
    cap={all ? Infinity : ROW_CAP}
    onShowAll={() => setAll(true)}
    className="mt-2 -mx-4 max-h-[300px] overflow-auto border-t border-border-subtle font-mono text-xs leading-5"
  />
}
