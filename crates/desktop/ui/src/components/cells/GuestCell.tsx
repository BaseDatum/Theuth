// Owner: U2
import { type ReactNode, useEffect, useSyncExternalStore } from 'react'
import type { Api } from '@/rpc/api'
import type { CellLatest } from '@/rpc/bindings/CellLatest'
import type { CellRef } from '@/rpc/bindings/CellRef'
import type { CellSpec } from '@/rpc/bindings/CellSpec'
import type { DocId } from '@/rpc/bindings/DocId'
import { Cell, CellExtras, Chip } from './Cell'
import { AnsiText } from './Outputs'
import { clock, duration } from './store'
import './note-cells.css'

export type GuestCellProps = {
  cell: CellSpec
  code: string
  /** `cell_latest` row for this cell, or none yet. */
  latest: CellLatest | null
  // 171:U2-ADD — the cell's identity and BlockNote's own code content (`contentRef`): an editor member still edits
  // the text in place (§13.1), and BlockNote needs its content mounted.
  cellRef: CellRef
  editor: ReactNode
}

const POLL_MS = 20_000

/** Per document: the replicated rows (`cell_latest`, §13.2) as last read, the subscribers, the mounted cells polling. */
type Slot = { rows: CellLatest[]; listeners: Set<() => void>; users: number; timer: number | null }
const latest = new Map<DocId, Slot>()

function slot(doc: DocId): Slot {
  let s = latest.get(doc)
  if (!s) latest.set(doc, (s = { rows: [], listeners: new Set(), users: 0, timer: null }))
  return s
}

/** Re-read the doc's rows; a refusal (an engine without `cell_latest`) keeps the last rows. */
async function read(api: Pick<Api, 'cell_latest'>, doc: DocId) {
  const s = slot(doc)
  s.rows = await api.cell_latest(doc).catch(() => s.rows)
  for (const l of s.listeners) l()
}

/**
 * The replicated latest row of one note cell (`"<doc>:<name or block id>"`), read through the guest's own api.
 * ponytail: guests get no cell events, so the rows are re-read every 20 s while a cell of the doc is mounted; a
 * `cell_latest` push on the guest channel would make it live.
 */
export function useLatest(api: Pick<Api, 'cell_latest'>, doc: DocId, cell: string): CellLatest | null {
  useEffect(() => {
    const s = slot(doc)
    if (s.users++ === 0) {
      void read(api, doc)
      s.timer = window.setInterval(() => void read(api, doc), POLL_MS)
    }
    return () => {
      if (--s.users === 0 && s.timer !== null) {
        clearInterval(s.timer)
        s.timer = null
      }
    }
  }, [api, doc])
  return useSyncExternalStore(
    (cb) => {
      const s = slot(doc)
      s.listeners.add(cb)
      return () => void s.listeners.delete(cb)
    },
    () => slot(doc).rows.find((r) => r.id === `${doc}:${cell}`) ?? null,
  )
}

/** The latest run's line: `exit 0 · 3 s · expect ✓ · latest run · owner · 14:31`. */
export function latestLine(row: CellLatest): string {
  const end = row.exit !== null ? `exit ${row.exit}` : row.state.replaceAll('_', ' ')
  return [end, row.secs !== null && duration(row.secs), row.expect, `latest run · owner · ${clock(Number(row.ended_ms))}`].filter(Boolean).join(' · ')
}

/**
 * 171 §13: a member's or guest's view — the code and chips in U1's `Cell` (view `guest`: no button, no keyboard
 * run, "Only the owner can run cells"), the latest replicated output as text only (images for guests are wave 3)
 * and `latest run · owner · 14:31`.
 */
export function GuestCell({ cell, code, latest: row, cellRef, editor }: GuestCellProps) {
  const ok = row !== null && (row.state === 'done' || (row.exit === 0 && row.state !== 'failed'))
  const below = row ? (
    <div className="flex flex-col gap-1 border-t border-border-subtle px-3 py-2">
      {row.text ? <AnsiText text={row.text} className="max-h-[28rem] overflow-auto text-xs whitespace-pre" /> : <div className="text-xs text-fg-tertiary">output kept private</div>}
      <div className="flex flex-wrap items-center gap-1.5 text-xs text-fg-secondary">
        <Chip tone={ok ? 'success' : 'danger'}>{ok ? '✓' : '✗'}</Chip>
        <span className="tabular-nums">{latestLine(row)}</span>
      </div>
      {row.summary && <div className="text-xs text-fg-tertiary">{row.summary}</div>}
    </div>
  ) : (
    // `div`, not `p`: BlockNote's paragraph styles reach every `p` inside a block.
    <div className="border-t border-border-subtle px-3 py-2 text-xs text-fg-tertiary">no run yet</div>
  )
  return (
    <div data-note-cell="" data-guest="">
      <CellExtras value={{ open: true, below }}>
        <Cell
          cellRef={cellRef}
          cell={cell}
          code={code}
          runnable={false}
          view="guest"
          state={null}
          latest={null}
          host={null}
          readOnly
          callbacks={{ onRun: () => undefined, onStop: () => undefined, onInput: () => undefined, onAsk: () => undefined, onEdit: () => undefined, onAttach: () => () => undefined }}
          editor={editor}
        />
      </CellExtras>
    </div>
  )
}
