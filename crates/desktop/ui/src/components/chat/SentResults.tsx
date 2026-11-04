import { ChevronRight } from 'lucide-react'
import { ResultCard } from '@/components/cells/ResultCard'
import type { CellResultInput } from '@/lib/injected'
import { useDisclosure } from '@/lib/tool-runs'

/**
 * 171F B4: the parked cell results a message carried into its turn, as one row above the bubble — collapsed by default,
 * expanding to the one-line result card per result (attributes only: a private cell's output never shows here).
 */
export function SentResults({ cells, disclosure }: { cells: CellResultInput[]; disclosure: string }) {
  const [open, setOpen] = useDisclosure(disclosure)
  return (
    <div className="flex flex-col items-end" onClick={(e) => e.stopPropagation()}>
      <button type="button" aria-expanded={open} className="flex min-h-11 items-center gap-1 text-xs text-fg-secondary hover:text-fg" onClick={() => setOpen(!open)}>
        <ChevronRight className={`size-3.5 transition-transform ${open ? 'rotate-90' : ''}`} aria-hidden />
        {cells.length === 1 ? '1 cell result sent with this message' : `${cells.length} cell results sent with this message`}
      </button>
      {open && cells.map((c) => <ResultCard key={c.run} run={c.run} name={c.name} label={c.label} cell={c.cell} exit={c.exit} secs={c.secs} />)}
    </div>
  )
}
