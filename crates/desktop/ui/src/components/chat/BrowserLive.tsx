import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { useApi } from '@/lib/rpc'
import { cn } from '@/lib/utils'

/** Live view of a session's headless browser: polls `browser_screenshot` twice a second while mounted (23 §3.2; the obscura guide's `captureScreenshot` loop, in-process). `fill` fits the image to its box (the tile). */
export function BrowserScreen({ session, fill = false }: { session: string; fill?: boolean }) {
  const canQuery1 = useQuerySupport(["browser_screenshot"])
  const api = useApi()
  const { data, isPending } = useQuery({
    queryKey: ['browser', session],
    queryFn: () => api.browser_screenshot(session),
    enabled: canQuery1,
    refetchInterval: 500,
    refetchIntervalInBackground: false,
  })
  return typeof data === 'string' ? (
    <img src={`data:image/png;base64,${data}`} alt="Live page" className={cn('rounded-lg', fill ? 'min-h-0 w-full flex-1 object-contain' : 'max-h-[85vh] w-auto')} />
  ) : (
    <p className="p-6 text-sm text-fg-secondary">{isPending ? 'Connecting…' : 'No live page — the browser is closed or idle.'}</p>
  )
}

/** The live browser in a dialog (phones and anywhere outside the pane tree); its content, and so the polling, mounts only while open. */
export function BrowserLive({ session, open, onOpenChange }: { session: string; open: boolean; onOpenChange(o: boolean): void }) {
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[90vw] p-2 pt-10 sm:max-w-[90vw]">
        <DialogTitle className="sr-only">Live browser</DialogTitle>
        <BrowserScreen session={session} />
      </DialogContent>
    </Dialog>
  )
}
