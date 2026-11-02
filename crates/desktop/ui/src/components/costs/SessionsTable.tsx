// 160 §7.10: root sessions in range (each with its subagents' calls), sortable, from the cube — no RPC. A row opens the
// session; its owner / project / model cells add filters instead.

import { useNavigate } from '@tanstack/react-router'
import { type MouseEvent, memo, useMemo, useState } from 'react'
import { BASES, BASIS_COLOR, BASIS_LABEL, type SessionRow } from '@/lib/costs/cube'
import { compact, int, usd, when } from '@/lib/costs/format'
import { cn } from '@/lib/utils'
import { OwnerGlyph, Panel, Swatch, Tag } from './ui'

type Col = 'label' | 'owner' | 'project' | 'calls' | 'tokens' | 'usd' | 'basis' | 'last'
const COLS: { key: Col | null; label: string; num?: boolean }[] = [
  { key: 'label', label: 'Session' },
  { key: 'owner', label: 'Owner' },
  { key: 'project', label: 'Project' },
  { key: null, label: 'Models' },
  { key: 'calls', label: 'Calls', num: true },
  { key: 'tokens', label: 'Tokens', num: true },
  { key: 'usd', label: 'Cost', num: true },
  { key: 'basis', label: 'Basis' },
  { key: 'last', label: 'Last call', num: true },
]
const PAGE = 50

/** The sortable value of a column; basis sorts by its rank in `BASES`. */
const sortKey = (r: SessionRow, c: Col): number | string => (c === 'basis' ? BASES.indexOf(r.basis) : c === 'project' ? (r.project ?? '') : r[c])

const TH = 'px-2 py-1.5 text-left font-mono text-[10px] font-medium tracking-[1.2px] whitespace-nowrap text-fg-tertiary uppercase select-none'
const TD = 'px-2 py-[7px] whitespace-nowrap'
const NUM = 'text-right font-mono tabular-nums'
const LINK = 'rounded-sm hover:text-foreground hover:underline focus-visible:ring-2 focus-visible:ring-ring/50 outline-none'

export const SessionsTable = memo(function SessionsTable({ rows, title, now, onFilter }: { rows: SessionRow[]; title: string; now: number; onFilter: (dim: 'owner' | 'model' | 'project', id: string) => void }) {
  const navigate = useNavigate()
  const [sort, setSort] = useState<{ col: Col; desc: boolean }>({ col: 'usd', desc: true })
  const [all, setAll] = useState(false)
  const max = rows[0]?.usd ?? 0
  const sorted = useMemo(() => {
    if (sort.col === 'usd' && sort.desc) return rows
    const s = sort.desc ? -1 : 1
    return [...rows].sort((a, b) => {
      const x = sortKey(a, sort.col)
      const y = sortKey(b, sort.col)
      return s * (typeof x === 'number' && typeof y === 'number' ? x - y : String(x).localeCompare(String(y))) || b.usd - a.usd
    })
  }, [rows, sort])
  const shown = all ? sorted : sorted.slice(0, PAGE)
  const open = (row: SessionRow) => {
    if (!row.deleted) void navigate({ to: '/c/$session', params: { session: row.id } })
  }
  const pick = (dim: 'owner' | 'model' | 'project', id: string) => (e: MouseEvent) => {
    e.stopPropagation()
    onFilter(dim, id)
  }
  return (
    <Panel title={title} note="root sessions with their subagents · click to open">
      {rows.length === 0 ? (
        <p className="py-2 text-[12px] text-fg-tertiary">No sessions in this range.</p>
      ) : (
        <div className="overflow-x-auto">
          <table className="w-full border-collapse text-[12.5px]">
            <thead>
              <tr className="border-b border-border-strong">
                {COLS.map((c) => {
                  const on = c.key === sort.col
                  return (
                    <th key={c.label} scope="col" aria-sort={on ? (sort.desc ? 'descending' : 'ascending') : undefined} className={cn(TH, c.num && NUM, c.key && 'cursor-pointer hover:text-foreground')}>
                      {c.key ? (
                        <button type="button" className="outline-none focus-visible:underline" onClick={() => setSort((s) => (s.col === c.key ? { col: s.col, desc: !s.desc } : { col: c.key!, desc: true }))}>
                          {c.label}
                          {on && <span aria-hidden> {sort.desc ? '▾' : '▴'}</span>}
                        </button>
                      ) : (
                        c.label
                      )}
                    </th>
                  )
                })}
              </tr>
            </thead>
            <tbody>
              {shown.map((row) => (
                <tr
                  key={row.id}
                  role={row.deleted ? undefined : 'link'}
                  tabIndex={row.deleted ? undefined : 0}
                  onClick={() => open(row)}
                  onKeyDown={(e) => e.key === 'Enter' && e.target === e.currentTarget && open(row)}
                  className={cn('border-b border-border outline-none hover:bg-layer-2/45 focus-visible:bg-layer-2/45', !row.deleted && 'cursor-pointer')}
                >
                  <td className={cn(TD, 'max-w-[280px] truncate', row.deleted ? 'text-fg-tertiary italic' : 'text-foreground')}>{row.label}</td>
                  <td className={TD}>
                    <button type="button" className={cn(LINK, 'inline-flex items-center gap-[5px] text-[11px] text-fg-secondary')} onClick={pick('owner', row.ownerId)}>
                      <OwnerGlyph kind={row.ownerKind} />
                      {row.owner}
                    </button>
                  </td>
                  <td className={cn(TD, 'text-[11px] text-fg-tertiary')}>
                    {row.project && (
                      <button type="button" className={LINK} onClick={pick('project', row.project)}>
                        {row.project}
                      </button>
                    )}
                  </td>
                  <td className={cn(TD, 'font-mono text-[11px] text-fg-secondary')}>
                    {row.models.slice(0, 1).map((m) => (
                      <button key={m} type="button" className={LINK} onClick={pick('model', m)}>
                        {m}
                      </button>
                    ))}
                    {row.models.length > 1 && (
                      <span className="ml-1.5 text-fg-tertiary" title={row.models.slice(1).join(', ')}>
                        +{row.models.length - 1}
                      </span>
                    )}
                  </td>
                  <td className={cn(TD, NUM)}>{int(row.calls)}</td>
                  <td className={cn(TD, NUM)}>{compact(row.tokens)}</td>
                  <td className={cn(TD, NUM)}>
                    <span aria-hidden className="mr-2 inline-block h-1 rounded-[2px] bg-[var(--chart-2)] align-middle opacity-85" style={{ width: max > 0 ? Math.round((row.usd / max) * 60) : 0 }} />
                    {usd(row.usd)}
                  </td>
                  <td className={TD}>
                    <Tag>
                      <Swatch color={BASIS_COLOR[row.basis]} className="mr-1" />
                      {BASIS_LABEL[row.basis]}
                    </Tag>
                  </td>
                  <td className={cn(TD, NUM, 'text-fg-tertiary')}>{when(row.last, now)}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {!all && sorted.length > PAGE && (
            <button type="button" className="mt-2 text-[12px] text-fg-secondary hover:text-foreground hover:underline" onClick={() => setAll(true)}>
              Show all {int(sorted.length)}
            </button>
          )}
        </div>
      )}
    </Panel>
  )
})
