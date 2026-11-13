import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { Dialog, DialogContent, DialogTitle } from '@/components/ui/dialog'
import { useApi } from '@/lib/rpc'

/** Live view of a session's headless browser: polls `browser_screenshot` twice a second while open (23 §3.2; the obscura guide's `captureScreenshot` loop, in-process). */
export function BrowserLive({ session, open, onOpenChange }: { session: string; open: boolean; onOpenChange(o: boolean): void }) {
  const canQuery1 = useQuerySupport(["browser_screenshot"])
  const api = useApi()
  const { data, isPending } = useQuery({
    queryKey: ['browser', session],
    queryFn: () => api.browser_screenshot(session),
    enabled: canQuery1 && (open),
    refetchInterval: open ? 500 : false,
    refetchIntervalInBackground: false,
  })
  return (
    <Dialog open={open} onOpenChange={onOpenChange}>
      <DialogContent className="max-w-[90vw] p-2 pt-10 sm:max-w-[90vw]">
        <DialogTitle className="sr-only">Live browser</DialogTitle>
        {typeof data === 'string' ? (
          <img src={`data:image/png;base64,${data}`} alt="Live page" className="max-h-[85vh] w-auto rounded-lg" />
        ) : (
          <p className="p-6 text-sm text-fg-secondary">{isPending ? 'Connecting…' : 'No live page — the browser is closed or idle.'}</p>
        )}
      </DialogContent>
    </Dialog>
  )
}
