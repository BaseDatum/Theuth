// #3: the `+` menu's prompt suffix steps — the saved suffixes (`SuffixList`), then the scope question for the picked
// one (`SuffixScopes`). Shared by every composer on every platform; on a phone or a touch screen the rows grow to a
// finger-sized target.

import { ChevronLeft, Settings2 } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { useIsMobile } from '@/hooks/use-mobile'
import { touch } from '@/lib/platform'
import type { Suffix, SuffixScope } from '@/lib/suffixes'
import { cn } from '@/lib/utils'

function useRow(): string {
  return useIsMobile() || touch() ? 'min-h-11' : ''
}

export function SuffixList({ suffixes, onPick, onBack, onManage }: { suffixes: Suffix[]; onPick(s: Suffix): void; onBack(): void; onManage(): void }) {
  const row = useRow()
  return (
    <div role="menu" aria-label="Prompt suffixes">
      <Button variant="ghost" className={cn('w-full justify-start text-fg-secondary', row)} onClick={onBack}>
        <ChevronLeft /> Add prompt suffix
      </Button>
      {suffixes.length === 0 && <p className="px-3 py-1.5 text-sm text-fg-tertiary">No suffixes saved yet.</p>}
      {suffixes.map((s) => (
        <Button key={s.id} variant="ghost" role="menuitem" data-suffix={s.id} className={cn('w-full justify-start', row)} onClick={() => onPick(s)}>
          <span className="truncate">{s.name}</span>
        </Button>
      ))}
      <div className="mt-1 border-t border-border-subtle pt-1">
        <Button variant="ghost" className={cn('w-full justify-start', row)} onClick={onManage}>
          <Settings2 /> Manage suffixes…
        </Button>
      </div>
    </div>
  )
}

const SCOPES = [
  ['once', 'This message'],
  ['always', 'Every message until I turn it off'],
] as const

export function SuffixScopes({ suffix, onPick }: { suffix: Suffix; onPick(scope: SuffixScope): void }) {
  const row = useRow()
  return (
    <div role="menu" aria-label={`Add ${suffix.name}`}>
      <p className="truncate px-3 py-1.5 text-xs text-fg-tertiary">Add “{suffix.name}” to</p>
      {SCOPES.map(([scope, label]) => (
        <Button
          key={scope}
          variant="ghost"
          role="menuitem"
          className={cn('h-auto w-full justify-start py-1.5 text-left whitespace-normal', row)}
          onClick={() => onPick(scope)}
        >
          {label}
        </Button>
      ))}
    </div>
  )
}
