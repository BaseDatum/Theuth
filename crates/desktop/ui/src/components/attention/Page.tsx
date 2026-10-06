import { Outlet, useParams } from '@tanstack/react-router'
import { ChevronDown, PanelLeft } from 'lucide-react'
import { useState } from 'react'
import { ATTENTION_EMPTY } from '@/components/hud/Hud'
import { useAttention } from '@/components/hud/queries'
import { SearchTrigger } from '@/components/sidebar/SearchTrigger'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { Skeleton } from '@/components/ui/skeleton'
import { isMacDesktop } from '@/lib/hotkeys'
import { cn } from '@/lib/utils'
import { groupItems } from './model'
import { Row } from './Row'
import { useSessionList } from './source'

/** 104 A4: the collapsed group keys, kept across launches. */
const GROUPS_KEY = 'attention.groups'

function readCollapsed(): Set<string> {
  try {
    const v: unknown = JSON.parse(localStorage.getItem(GROUPS_KEY) ?? '[]')
    return new Set(Array.isArray(v) ? v.filter((x): x is string => typeof x === 'string') : [])
  } catch {
    return new Set()
  }
}

/**
 * `/attention` (101 D6, 104 A4): the 52 px Attention header, then a 320 px rail of inbox rows grouped by kind (headers
 * with counts, collapsible, newest first inside) beside the selected item's screen (`<Outlet/>`). Under `md` one pane
 * at a time: the rows at `/attention`, the screen at `/attention/$id` (which carries its own header with the back chevron).
 */
export function Page() {
  const sidebar = useSidebar()
  const mac = isMacDesktop()
  const attention = useAttention()
  const sessions = useSessionList()
  const { id: active } = useParams({ strict: false })
  const items = attention.data
  const [collapsed, setCollapsed] = useState(readCollapsed)
  const toggle = (key: string) =>
    setCollapsed((prev) => {
      const next = new Set(prev)
      if (!next.delete(key)) next.add(key)
      localStorage.setItem(GROUPS_KEY, JSON.stringify([...next]))
      return next
    })
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <header
        data-tauri-drag-region={mac || undefined}
        className={cn('h-[52px] shrink-0 items-center gap-1 border-b border-border-subtle px-3', !sidebar.open && mac && 'pl-20', active ? 'hidden md:flex' : 'flex')}
      >
        {(!sidebar.open || sidebar.isMobile) && (
          <div className="flex shrink-0 items-center">
            <SearchTrigger />
            <Button variant="ghost" size="icon-sm" aria-label="Show sidebar" onClick={sidebar.toggleSidebar}>
              <PanelLeft />
            </Button>
          </div>
        )}
        <h2 className="min-w-0 flex-1 truncate text-sm font-semibold">
          Attention {items && <span className="font-normal text-fg-tertiary tabular-nums">{items.length}</span>}
        </h2>
      </header>
      <div className="flex min-h-0 flex-1 overflow-hidden">
        <aside className={cn('w-full shrink-0 flex-col overflow-y-auto border-r border-border-subtle md:flex md:w-[320px]', active ? 'hidden' : 'flex')}>
          {attention.isPending ? (
            <div className="flex flex-col gap-3 p-3">
              <Skeleton className="h-14 w-full" />
              <Skeleton className="h-14 w-full" />
              <Skeleton className="h-14 w-full" />
            </div>
          ) : attention.isError ? (
            <p className="p-3 text-sm text-danger">{attention.error.message}</p>
          ) : items && items.length > 0 ? (
            groupItems(items).map((g) => (
              <section key={g.key} aria-label={g.label}>
                <button
                  type="button"
                  aria-expanded={!collapsed.has(g.key)}
                  onClick={() => toggle(g.key)}
                  className="sticky top-0 z-10 flex h-11 w-full items-center gap-2 border-b border-border-subtle bg-background px-3 text-xs font-semibold uppercase tracking-wide text-fg-secondary hover:bg-accent/60"
                >
                  <ChevronDown className={cn('size-3.5 transition-transform', collapsed.has(g.key) && '-rotate-90')} />
                  {g.label}
                  <span className="font-normal tabular-nums text-fg-tertiary">{g.items.length}</span>
                </button>
                {!collapsed.has(g.key) && (
                  <ul className="divide-y divide-border-subtle border-b border-border-subtle">
                    {g.items.map((i) => (
                      <Row key={i.id} item={i} active={i.id === active} sessions={sessions.data} />
                    ))}
                  </ul>
                )}
              </section>
            ))
          ) : (
            <p className="m-3 rounded-lg border border-dashed border-border-subtle px-3 py-3 text-sm text-fg-tertiary">{ATTENTION_EMPTY}</p>
          )}
        </aside>
        <main className={cn('min-h-0 min-w-0 flex-1 flex-col md:flex', active ? 'flex' : 'hidden')}>
          <Outlet />
        </main>
      </div>
    </article>
  )
}
