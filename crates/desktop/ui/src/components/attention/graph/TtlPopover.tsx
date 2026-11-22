// 208 §8: the TTL settings — minutes per kind, applied at once (the view re-derives; the window widens if needed).
import { Timer } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { useGraphPrefs } from './prefs'
import { TTL_DEFAULTS, type TtlPrefs } from './ttl'

const ROWS: { key: keyof TtlPrefs; label: string }[] = [
  { key: 'child_min', label: 'Subagents' },
  { key: 'root_min', label: 'Chats and peers' },
  { key: 'desk_min', label: 'Desks' },
  { key: 'edge_min', label: 'Messages stay lit' },
]

export function TtlPopover() {
  const ttl = useGraphPrefs((s) => s.ttl)
  const set = useGraphPrefs((s) => s.set)
  return (
    <Popover>
      <PopoverTrigger asChild>
        <Button size="icon-xs" variant="ghost" aria-label="Fade times" title="Fade times">
          <Timer />
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-64 p-3 text-xs">
        <p className="mb-1 font-medium text-sm">Fade after (minutes)</p>
        <p className="mb-2 text-fg-tertiary">Counted from each node's last event. Working and waiting nodes never fade.</p>
        <div className="flex flex-col gap-1.5">
          {ROWS.map((r) => (
            <label key={r.key} className="flex items-center justify-between gap-2">
              <span className="text-fg-secondary">{r.label}</span>
              <Input
                type="number"
                dictate={false}
                min={1}
                value={ttl[r.key]}
                onChange={(e) => {
                  const n = Number(e.target.value)
                  if (Number.isFinite(n) && n > 0) set({ ttl: { ...ttl, [r.key]: n } })
                }}
                className="h-7 w-20 text-right tabular-nums"
              />
            </label>
          ))}
        </div>
        <Button size="xs" variant="ghost" className="mt-2" onClick={() => set({ ttl: TTL_DEFAULTS })}>
          Reset
        </Button>
      </PopoverContent>
    </Popover>
  )
}
