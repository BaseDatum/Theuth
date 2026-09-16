import { useMutation, useQueryClient } from '@tanstack/react-query'
import { Outlet, getRouteApi, useParams } from '@tanstack/react-router'
import { PanelLeft, Plus, RefreshCw } from 'lucide-react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { NewDocDialog } from '@/components/knowledge/NewDocDialog'
import { SearchBox } from '@/components/knowledge/SearchBox'
import { Tree } from '@/components/knowledge/Tree'
import { Button } from '@/components/ui/button'
import { useSidebar } from '@/components/ui/sidebar'
import { Tooltip, TooltipContent, TooltipTrigger } from '@/components/ui/tooltip'
import { isMacDesktop } from '@/lib/hotkeys'
import { queryKeys, useApi } from '@/lib/rpc'
import { cn } from 'cn'

const route = getRouteApi('/knowledge')

/** `/knowledge`: 300 px rail (search, tree) and the document outlet (25 §8). Under `md` one pane at a time: the tree at `/knowledge`, the document at `/knowledge/$id`. */
export function Explorer() {
  const api = useApi()
  const qc = useQueryClient()
  const { q } = route.useSearch()
  const { _splat: active } = useParams({ strict: false })
  const sidebar = useSidebar()
  const [creating, setCreating] = useState(false)

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

  const mac = isMacDesktop()
  return (
    <div className="flex min-h-0 flex-1 overflow-hidden">
      <aside
        className={cn(
          'w-full shrink-0 flex-col border-r border-border-subtle md:flex md:w-[300px]',
          active ? 'hidden' : 'flex',
        )}
      >
        <header
          data-tauri-drag-region={mac || undefined}
          className={cn(
            'flex h-[52px] shrink-0 items-center gap-1 px-3',
            !sidebar.open && mac && 'pl-20',
          )}
        >
          {(!sidebar.open || sidebar.isMobile) && (
            <Button
              variant="ghost"
              size="icon-sm"
              aria-label="Show sidebar"
              onClick={sidebar.toggleSidebar}
            >
              <PanelLeft />
            </Button>
          )}
          <h2 className="flex-1 truncate text-sm font-semibold">Knowledge</h2>
          <Tooltip>
            <TooltipTrigger asChild>
              <Button
                variant="ghost"
                size="icon-sm"
                aria-label="Sync"
                disabled={sync.isPending}
                onClick={() =>
                  sync.mutate(undefined, {
                    onSuccess: (r) =>
                      toast(`Synced: ${r.added} added, ${r.updated} updated, ${r.removed} removed`),
                    onError: (e) => toast.error(e.message),
                  })
                }
              >
                <RefreshCw className={sync.isPending ? 'animate-spin' : undefined} />
              </Button>
            </TooltipTrigger>
            <TooltipContent>Sync from disk</TooltipContent>
          </Tooltip>
          <Button variant="ghost" size="sm" onClick={() => setCreating(true)}>
            <Plus />
            New
          </Button>
        </header>
        <SearchBox initial={q ?? ''}>
          <Tree active={active} />
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
      <NewDocDialog open={creating} onOpenChange={setCreating} />
    </div>
  )
}
