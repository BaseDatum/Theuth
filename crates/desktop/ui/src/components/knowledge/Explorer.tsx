import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Outlet, getRouteApi, useParams } from '@tanstack/react-router'
import { History as HistoryIcon, Moon, Plus, RefreshCw } from 'lucide-react'
import { type ReactNode, useEffect, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { History } from '@/components/knowledge/History'
import { NewDocDialog } from '@/components/knowledge/NewDocDialog'
import { SearchBox } from '@/components/knowledge/SearchBox'
import { Tree } from '@/components/knowledge/Tree'
import { SectionBand } from '@/components/nav/SectionBand'
import { Button } from '@/components/ui/button'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from 'cn'

const route = getRouteApi('/knowledge')

/**
 * `/knowledge`: the Library band over two panes (97b) — a 300 px rail (search + Sync · History · New, then the tree)
 * and the document outlet (25 §8). Under `md` one pane at a time: the tree at `/knowledge`, the document at
 * `/knowledge/$id`. `?history=true` at bare `/knowledge` opens the whole bundle's History sheet (the composer's
 * `/knowledge history`); with a document open the same flag belongs to that document's sheet.
 */
export function Explorer() {
  const api = useApi()
  const qc = useQueryClient()
  const navigate = route.useNavigate()
  const { q, history } = route.useSearch()
  const { _splat: active } = useParams({ strict: false })
  const [creating, setCreating] = useState(false)
  // 136 §8: the "changed by dreaming" filter — `source: dreaming` or a status other than `tidy`.
  const [dreaming, setDreaming] = useState(false)
  // The rail's History opens beside whatever document is shown; only the bare route's `?history=true` is URL state.
  const [historyLocal, setHistoryLocal] = useState(false)
  const historyOpen = historyLocal || (!active && history === true)
  const setHistoryOpen = (open: boolean) => {
    setHistoryLocal(open)
    if (!open && history === true && !active)
      navigate({ to: '/knowledge', replace: true, search: ({ history: _history, ...rest }) => rest })
  }

  const sync = useMutation({
    mutationFn: () => api.knowledge_sync(),
    onSettled: () => qc.invalidateQueries({ queryKey: queryKeys.knowledgeList }),
  })
  const { mutate: runSync } = sync

  useEffect(() => {
    runSync()
    const onFocus = () => runSync()
    window.addEventListener('focus', onFocus)
    return () => window.removeEventListener('focus', onFocus)
  }, [runSync])

  const action = (label: string, icon: ReactNode, onClick: () => void, disabled = false, pressed?: boolean) => (
    <Tooltip>
      <TooltipTrigger asChild>
        <Button variant="ghost" size="icon-sm" aria-label={label} aria-pressed={pressed} disabled={disabled} className={pressed ? 'bg-layer-3' : undefined} onClick={onClick}>
          {icon}
        </Button>
      </TooltipTrigger>
      <TooltipContent>{label}</TooltipContent>
    </Tooltip>
  )
  return (
    <article className="flex min-h-0 min-w-0 flex-1 flex-col">
      <SectionBand group="library" />
      <div className="flex min-h-0 flex-1 overflow-hidden">
        <aside
          className={cn(
            'w-full shrink-0 flex-col border-r border-border-subtle md:flex md:w-[300px]',
            active ? 'hidden' : 'flex',
          )}
        >
          <SearchBox
            initial={q ?? ''}
            actions={
              <>
                {action(
                  'Sync from disk',
                  <RefreshCw className={sync.isPending ? 'animate-spin' : undefined} />,
                  () =>
                    sync.mutate(undefined, {
                      onSuccess: (r) => toast(`Synced: ${r.added} added, ${r.updated} updated, ${r.removed} removed`),
                      onError: (e) => toast.error(e.message),
                    }),
                  sync.isPending,
                )}
                {action('Changed by dreaming', <Moon />, () => setDreaming((v) => !v), false, dreaming)}
                {action('History', <HistoryIcon />, () => setHistoryOpen(true))}
                {action('New document', <Plus />, () => setCreating(true))}
              </>
            }
          >
            <Tree active={active} dreaming={dreaming} />
          </SearchBox>
        </aside>
        <main className={cn('min-w-0 flex-1 md:flex', active ? 'flex' : 'hidden')}>
          {/* The `$` child also matches bare `/knowledge`, with an empty splat. */}
          {!active ? (
            <p className="m-auto text-sm text-fg-tertiary">Select a document</p>
          ) : (
            <Outlet />
          )}
        </main>
      </div>
      <NewDocDialog open={creating} onOpenChange={setCreating} />
      <History id={null} open={historyOpen} onOpenChange={setHistoryOpen} />
    </article>
  )
}
