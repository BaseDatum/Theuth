// CELLAFTER (Sky, 2026-11-07): the ask before a danger cell's line or selection ▶ runs. An inline box positioned under
// the command, never a modal: Radix's `hideOthers` (and any `aria-live`) inside a note's node view loops ProseMirror
// (CELLHIST). It closes on Cancel, Esc, a press outside it, or (the hosts) any change of the cell's text.
import { useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'

/** Bottom padding a host adds while the confirm is open, so a press on its last line is not clipped. */
export const CONFIRM_ROOM = 'pb-32'

export type LineConfirmProps = {
  /** Offset of the command's LAST line in the positioned parent (px): the box sits under it, never over it. */
  top: number
  what: 'line' | 'selection'
  /** The command's first and last code line. */
  lines: [number, number]
  /** The cell's title (label, else name). */
  title: string
  /** The unarmed `after` gate's reason when the cell is also disarmed (one ask covers both). */
  reason: string | null
  /** `skip`: "Don't ask again for this cell" was ticked. */
  onRun(skip: boolean): void
  onCancel(): void
}

/** The question and the run button's text: one line, a multi-line command, or a selection. */
export function confirmText(what: 'line' | 'selection', [a, b]: [number, number], title: string): { question: string; action: string } {
  if (what === 'selection') return { question: `Run this selection of ${title}?`, action: 'Run selection' }
  return a === b ? { question: `Run this line of ${title}?`, action: 'Run line' } : { question: `Run lines ${a}–${b} of ${title}?`, action: 'Run lines' }
}

export function LineConfirm({ top, what, lines, title, reason, onRun, onCancel }: LineConfirmProps) {
  const [skip, setSkip] = useState(false)
  const box = useRef<HTMLDivElement>(null)
  const cancel = useRef(onCancel)
  cancel.current = onCancel
  useEffect(() => {
    // S1: focus the safe choice, without scrolling (the cell's overflow box would shift its content under the header).
    box.current?.querySelector<HTMLButtonElement>('[data-cancel]')?.focus({ preventScroll: true })
    // M1: a press anywhere else closes it.
    const outside = (e: MouseEvent) => e.target instanceof Node && !box.current?.contains(e.target) && cancel.current()
    document.addEventListener('mousedown', outside, true)
    return () => document.removeEventListener('mousedown', outside, true)
  }, [])
  const { question, action } = confirmText(what, lines, title)
  return (
    <div
      ref={box}
      role="dialog"
      aria-label={question}
      contentEditable={false}
      className="absolute left-8 z-20 flex max-w-sm flex-col gap-2 rounded-md border border-danger bg-layer-1 p-2.5 text-xs shadow-md"
      // Just under the command's last line, which stays readable; the host pads its bottom (`CONFIRM_ROOM`).
      style={{ top: top + 22 }}
      onMouseDown={(e) => e.stopPropagation()}
      onKeyDown={(e) => {
        // Keys stay in the box: Esc cancels; Enter/Space press the focused button, never the editor.
        e.stopPropagation()
        if (e.key === 'Escape') {
          e.preventDefault()
          onCancel()
        }
      }}
    >
      <p className="font-medium text-danger">{question}</p>
      {reason && <p className="text-warning">{reason}</p>}
      <label className="flex items-center gap-1.5 text-fg-secondary">
        <input type="checkbox" checked={skip} onChange={(e) => setSkip(e.target.checked)} />
        Don&apos;t ask again for this cell
      </label>
      <div className="flex justify-end gap-1.5">
        <Button size="sm" variant="ghost" className="h-7 pointer-coarse:h-12" data-cancel onClick={onCancel}>
          Cancel
        </Button>
        <Button size="sm" variant="destructive" className="h-7 pointer-coarse:h-12" onClick={() => onRun(skip)}>
          {action}
        </Button>
      </div>
    </div>
  )
}
