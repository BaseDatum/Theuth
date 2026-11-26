// The costs header's refresh button: everything the page shows, fetched again at once (`useRefreshCosts`), with the
// time of the last refresh beside it.

import { RefreshCw } from 'lucide-react'
import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { ago } from '@/lib/costs/format'
import { useRefreshCosts } from '@/lib/costs/queries'

/** The label's clock while a refresh time is shown and the page is on screen. */
const TICK_MS = 10_000

export function RefreshButton({ visible }: { visible: boolean }) {
  const refresh = useRefreshCosts()
  const [running, setRunning] = useState(false)
  const [at, setAt] = useState<number | null>(null)
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (at === null || !visible) return
    setNow(Date.now())
    const t = setInterval(() => setNow(Date.now()), TICK_MS)
    return () => clearInterval(t)
  }, [at, visible])
  const run = () => {
    setRunning(true)
    void refresh().finally(() => {
      const t = Date.now()
      setAt(t)
      setNow(t)
      setRunning(false)
    })
  }
  return (
    <>
      {at !== null && (
        <span className="hidden font-mono text-[10.5px] whitespace-nowrap text-fg-tertiary sm:inline" title={new Date(at).toLocaleString()}>
          {running ? 'Updating…' : `Updated ${ago(at, now)}`}
        </span>
      )}
      <Button variant="outline" size="sm" onClick={run} disabled={running} aria-busy={running} title="Fetch costs, ledger and usage limits again now">
        <RefreshCw className={running ? 'animate-spin' : undefined} />
        Refresh
      </Button>
    </>
  )
}
