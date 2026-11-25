// 216 §11, contract §12.6: a plugin's log today (`plugin_logs(name, 200)`, already redacted by the host) in a side sheet,
// with a Refresh. Fetched only while the sheet is open.
import { useQuery } from '@tanstack/react-query'
import { RefreshCw } from 'lucide-react'
import { errorText } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { queryKeys, useApi } from '@/lib/rpc'

/** The last lines Settings asks for (the host caps a tail at 2000). */
export const LOG_TAIL = 200

export function LogsSheet({ name, open, onOpenChange }: { name: string; open: boolean; onOpenChange: (open: boolean) => void }) {
  const api = useApi()
  const logs = useQuery({ enabled: open, queryKey: queryKeys.pluginLogs(name), queryFn: () => api.plugin_logs(name, LOG_TAIL) })
  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="flex w-full flex-col gap-0 overflow-hidden p-0 sm:max-w-2xl">
        <SheetHeader className="shrink-0 border-b border-border-subtle px-5 pt-5 pb-3">
          <SheetTitle className="flex items-center gap-2 pr-8 text-base">
            <span className="min-w-0 flex-1 truncate">Log of {name}</span>
            <Button variant="ghost" size="sm" disabled={logs.isFetching} onClick={() => void logs.refetch()}>
              <RefreshCw className={logs.isFetching ? 'animate-spin' : undefined} />
              Refresh
            </Button>
          </SheetTitle>
          <SheetDescription className="truncate font-mono text-xs" title={logs.data?.path}>
            {logs.data?.path ?? (logs.error ? errorText(logs.error) : `Today’s log, last ${LOG_TAIL} lines`)}
          </SheetDescription>
        </SheetHeader>
        <div className="min-h-0 flex-1 overflow-auto px-5 py-4">
          {logs.data && logs.data.lines.length === 0 && <p className="text-sm text-fg-tertiary">Nothing logged today.</p>}
          {logs.data && logs.data.lines.length > 0 && <pre className="font-mono text-xs leading-relaxed whitespace-pre-wrap break-all text-fg-secondary">{logs.data.lines.join('\n')}</pre>}
        </div>
      </SheetContent>
    </Sheet>
  )
}
