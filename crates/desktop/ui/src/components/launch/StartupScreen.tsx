import { ChevronRight } from 'lucide-react'
import { useEffect, useId, useRef, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { FailureText, useLinkFailure } from '@/components/link/LinkFailure'
import { PairScreen } from '@/components/link/PairScreen'
import { Button } from '@/components/ui/button'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { isMacDesktop } from '@/lib/hotkeys'
import { INITIAL, diagnostics } from '@/lib/link-status'
import { connection, launchLog, onLaunchLog } from '@/lib/tauri'
import { disclosureKey, useDisclosure } from '@/lib/tool-runs'
import { LOCAL, hasLocal, useEngines } from '@/link/engines'
import { useConnection } from '@/store/connection'
import { LaunchCard, StepList, StepRow } from './LaunchCard'
import { STUCK_MS, type StartupView, cleanLog, startupView } from './startup-progress'

type Props = {
  onRelaunch: () => void
  onQuit: () => void
}

/** The local daemon's launch screen, or the link overlay when the active engine is remote (31 §9). */
export function StartupScreen(props: Props) {
  const active = useEngines((s) => s.active)
  return active === LOCAL ? <LocalStartup {...props} /> : <LinkStartup id={active} />
}

/**
 * A paired engine that is not reachable, in the shutdown card's look: one step reaching it (the presence hint, or the
 * named failure under it). No Quit/Relaunch (those act on the desktop's own daemon); every way out — another engine,
 * pairing a new one, or forgetting this entry (a phone has no other route to those while nothing connects).
 * Rendered outside the router, so switching is `setActive`, not a route.
 */
function LinkStartup({ id }: { id: string }) {
  const { engines, presence: all, setActive, remove } = useEngines()
  const [pairing, setPairing] = useState<'new' | 'repair' | null>(null)
  const titleId = useId()
  const row = engines.find((h) => h.id === id)
  const mesh = row?.kind === 'mesh'
  const infoName = useConnection((s) => s.info?.name)
  const name = (mesh && row.meshName) || infoName || row?.name || id
  const failure = useLinkFailure(id)
  // ponytail: one button to the local daemon, else another paired engine (online first); a picker if people pair many.
  const others = engines.filter((h) => h.id !== id)
  const next = hasLocal() ? LOCAL : (others.find((h) => all[h.id] === 'online') ?? others[0])?.id

  if (pairing) return <PairScreen replace={pairing === 'repair' ? row : undefined} onPaired={() => setPairing(null)} onBack={() => setPairing(null)} />

  const copyDiagnostics = () =>
    row &&
    navigator.clipboard
      .writeText(diagnostics(useEngines.getState().link[id] ?? INITIAL, Date.now(), row))
      .then(() => toast.message('Copied'), () => toast.error('Copy failed'))
  // `presence === 'refused'` is `failure.kind === 'revoked'` now; the plain text below covers a first connect and short blips.
  const detail = mesh
    ? 'The hub is offline or the relay is unreachable. Another node of the mesh takes over within about half a minute; this screen closes when it answers.'
    : 'The node is offline or the relay is unreachable. This screen closes when it is back.'
  return (
    <div
      className="flex h-band items-center justify-center bg-background px-4 text-foreground"
      data-tauri-drag-region={isMacDesktop() || undefined}
    >
      <LaunchCard role="region" aria-labelledby={titleId} title={`Connecting to ${name}`} titleId={titleId}>
        <StepList>
          <StepRow step={{ id: 'reach', label: `Reaching ${name}`, status: failure ? 'failed' : 'active', detail: failure ? undefined : detail }}>
            {failure && <FailureText failure={failure} onRepair={() => setPairing('repair')} className="text-sm" />}
          </StepRow>
        </StepList>
        <div className="flex flex-wrap justify-end gap-2">
          <Button variant="ghost" className="min-h-11" onClick={() => void copyDiagnostics()}>
            Copy diagnostics
          </Button>
          <Button variant="ghost" className="min-h-11" onClick={() => remove(id)}>
            Forget this node
          </Button>
          <Button variant="secondary" className="min-h-11" onClick={() => setPairing('new')}>
            Pair with a mesh…
          </Button>
          {next !== undefined && (
            <Button variant="secondary" className="min-h-11" onClick={() => setActive(next)}>
              Switch node
            </Button>
          )}
        </div>
      </LaunchCard>
    </div>
  )
}

/**
 * Shown until the socket is open and `info()` answered (25 §4.3): the start as steps (`startup-progress.ts`), the pty
 * transcript one click away (the diagnostic when a shell rc waits for input, 18 §4), and Relaunch / Quit.
 */
function LocalStartup({ onRelaunch, onQuit }: Props) {
  const state = useConnection((s) => s.state)
  const started = useConnection((s) => s.client !== null)
  const info = useConnection((s) => s.info !== null)
  const error = useConnection((s) => s.error)
  const [log, setLog] = useState('')
  const [owned, setOwned] = useState<boolean | null>(null)
  const [since] = useState(Date.now)
  const [now, setNow] = useState(since)

  useEffect(() => {
    let live = true
    const unlisten = onLaunchLog((chunk) => setLog((l) => l + chunk))
    void launchLog().then((initial) => {
      if (live) setLog(initial)
    })
    return () => {
      live = false
      void unlisten.then((f) => f())
    }
  }, [])

  // The shell caches the connection once it answered (launch.rs `Launch::connection`): asking again reads `owned`
  // without starting anything.
  useEffect(() => {
    if (!started) return
    let live = true
    connection().then((c) => live && setOwned(c.owned), () => undefined)
    return () => {
      live = false
    }
  }, [started])

  // One tick at `STUCK_MS` while the engine step is active; `waited` keeps it after, so the log stays open.
  useEffect(() => {
    if (started) return
    const t = setTimeout(() => setNow(Date.now()), Math.max(0, since + STUCK_MS - Date.now()))
    return () => clearTimeout(t)
  }, [started, since])

  const view = startupView({ started, owned, state, info, error, log, waited: now - since })
  return (
    <LocalStartupView
      view={view}
      log={log}
      onQuit={onQuit}
      onRelaunch={() => {
        setLog('')
        onRelaunch()
      }}
    />
  )
}

/** The launch log's open state outlives a remount of the screen (`useDisclosure`); `logOpen` decides until the user toggles. */
const LOG_KEY = disclosureKey('startup', 'launch-log')

export function LocalStartupView({ view, log, onQuit, onRelaunch }: { view: StartupView; log: string } & Props) {
  const titleId = useId()
  const [open, setOpen] = useDisclosure(LOG_KEY, view.logOpen)
  const pre = useRef<HTMLPreElement>(null)
  const text = cleanLog(log)

  useEffect(() => {
    pre.current?.scrollTo(0, pre.current.scrollHeight)
  }, [text, open])

  return (
    <div
      className="flex h-dvh items-center justify-center bg-background p-4 text-foreground"
      data-tauri-drag-region={isMacDesktop() || undefined}
    >
      <LaunchCard role="region" aria-labelledby={titleId} title={view.title} titleId={titleId} failed={view.failed}>
        <StepList>
          {view.steps.map((step) => (
            <StepRow key={step.id} step={step} />
          ))}
        </StepList>
        {text && (
          <Collapsible open={open} onOpenChange={setOpen} className="flex flex-col gap-2">
            <CollapsibleTrigger className="flex w-fit items-center gap-1 text-[13px] text-fg-secondary [&[data-state=open]>svg]:rotate-90">
              <ChevronRight className="size-3.5 transition-transform" aria-hidden />
              {open ? 'Hide launch log' : 'Show launch log'}
            </CollapsibleTrigger>
            <CollapsibleContent>
              <pre
                ref={pre}
                className="max-h-[50vh] overflow-auto rounded-md bg-layer-2 p-3 font-mono text-xs leading-5 whitespace-pre-wrap break-all text-fg-secondary"
              >
                {text}
              </pre>
            </CollapsibleContent>
          </Collapsible>
        )}
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onQuit}>
            Quit
          </Button>
          {view.relaunch && <Button onClick={onRelaunch}>Relaunch</Button>}
        </div>
      </LaunchCard>
    </div>
  )
}
