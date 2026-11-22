import { ChevronUp, Globe } from 'lucide-react'
import { useMemo, useState } from 'react'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { useIsMobile } from '@/hooks/use-mobile'
import { pillOf, restoreWeb, useWebViews, type WebView } from '@/lib/web-views'

/** The page's origin when it is http(s), else `null`. */
function originOf(url: string): string | null {
  try {
    const u = new URL(url)
    return u.protocol === 'http:' || u.protocol === 'https:' ? u.origin : null
  } catch {
    return null
  }
}

/** The origin's `/favicon.ico` as an image, the globe when there is none (or it fails): no other request. */
function Favicon({ url }: { url: string }) {
  const origin = originOf(url)
  const [failed, setFailed] = useState<string | null>(null)
  if (!origin || failed === origin) return <Globe className="size-4 shrink-0 text-fg-secondary" aria-hidden="true" />
  return <img src={`${origin}/favicon.ico`} alt="" aria-hidden="true" referrerPolicy="no-referrer" className="size-4 shrink-0 rounded-sm" onError={() => setFailed(origin)} />
}

const label = (v: WebView) => v.title || originOf(v.current || v.url)?.replace(/^https?:\/\//, '') || 'Web page'

/**
 * 209 §7: `session`'s minimised web pages, right above its composer on a phone: the latest one's favicon and title (a
 * tap brings it back full screen, as it was: its native view was only hidden) and, with several, their count, which
 * lists them all to restore any. Another chat's pages are not here. Nothing on a tablet or desktop, where pages are
 * tiles.
 */
export function WebPill({ session }: { session: string }) {
  const phone = useIsMobile()
  const views = useWebViews((s) => s.views)
  const pill = useMemo(() => pillOf(views, session), [views, session])
  const [listing, setListing] = useState(false)
  if (!phone || !pill) return null
  const { latest, count, list } = pill
  return (
    <div className="mx-auto flex w-full shrink-0 items-center gap-2 px-4 pb-2" data-web-pill="">
      <Button variant="outline" className="h-10 min-w-0 flex-1 justify-start gap-2 rounded-full px-3" aria-label={`Show ${label(latest)}`} onClick={() => restoreWeb(latest.id)}>
        <Favicon key={latest.id} url={latest.current || latest.url} />
        <span className="min-w-0 flex-1 truncate text-left">{label(latest)}</span>
        <ChevronUp className="size-4 shrink-0 text-fg-secondary" aria-hidden="true" />
      </Button>
      {count > 1 && (
        <Popover open={listing} onOpenChange={setListing}>
          <PopoverTrigger asChild>
            <Button variant="outline" className="h-10 min-w-10 rounded-full px-3 tabular-nums" aria-label={`${count} web pages`}>
              {count}
            </Button>
          </PopoverTrigger>
          <PopoverContent side="top" align="end" className="w-72 p-1">
            <ul aria-label="Web pages" className="flex flex-col">
              {list.map((v) => (
                <li key={v.id}>
                  <Button
                    variant="ghost"
                    className="h-11 w-full justify-start gap-2 px-2"
                    onClick={() => {
                      setListing(false)
                      restoreWeb(v.id)
                    }}
                  >
                    <Favicon url={v.current || v.url} />
                    <span className="min-w-0 flex-1 truncate text-left">{label(v)}</span>
                  </Button>
                </li>
              ))}
            </ul>
          </PopoverContent>
        </Popover>
      )}
    </div>
  )
}
