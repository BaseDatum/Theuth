import { useEffect, useRef, useState } from 'react'
import { PairScreen } from '@/components/link/PairScreen'
import { Button } from '@/components/ui/button'
import { isMacDesktop } from '@/lib/hotkeys'
import { launchLog, onLaunchLog } from '@/lib/tauri'
import { LOCAL, hasLocal, useHarnesses } from '@/link/harnesses'
import { useConnection } from '@/store/connection'

type Props = {
  onRelaunch: () => void
  onQuit: () => void
}

/** The local daemon's launch screen, or the link overlay when the active harness is remote (31 §9). */
export function StartupScreen(props: Props) {
  const active = useHarnesses((s) => s.active)
  return active === LOCAL ? <LocalStartup {...props} /> : <LinkStartup id={active} />
}

/**
 * A paired harness that is not reachable: no Quit/Relaunch (those act on the desktop's own
 * daemon), the presence hint, and every way out — another harness, pairing a new one, or
 * forgetting this entry (a phone has no other route to those while nothing connects).
 * Rendered outside the router, so switching is `setActive`, not a route.
 */
function LinkStartup({ id }: { id: string }) {
  const { harnesses, presence: all, setActive, remove } = useHarnesses()
  const [pairing, setPairing] = useState(false)
  const presence = all[id]
  const name = useConnection((s) => s.info?.name) ?? harnesses.find((h) => h.id === id)?.name ?? id
  // ponytail: one button to the local daemon, else another paired harness (online first); a picker if people pair many.
  const others = harnesses.filter((h) => h.id !== id)
  const next = hasLocal() ? LOCAL : (others.find((h) => all[h.id] === 'online') ?? others[0])?.id

  if (pairing) return <PairScreen onPaired={() => setPairing(false)} onBack={() => setPairing(false)} />

  const refused = presence === 'refused'
  return (
    <div
      className="flex h-band items-center justify-center bg-background px-4 text-foreground"
      data-tauri-drag-region={isMacDesktop() || undefined}
    >
      <div className="flex w-full max-w-[640px] flex-col gap-4 rounded-xl border border-border-subtle bg-elevated p-6">
        <p className="text-fg-secondary">{refused ? `${name} refused this device` : `Reconnecting to ${name}…`}</p>
        {presence === 'offline' && <p className="text-xs text-fg-secondary">Harness offline</p>}
        {presence === 'connecting' && <p className="text-xs text-fg-secondary">Relay unreachable</p>}
        <p className="text-sm text-fg-secondary">
          {refused
            ? 'The harness no longer knows this device (revoked, or its device list was reset). Show a new pairing code there (Settings › Link, or `wth --link`), pair again, then forget this entry.'
            : 'The harness is offline or the relay is unreachable. This screen closes when it is back.'}
        </p>
        <div className="flex flex-wrap justify-end gap-2">
          <Button variant="ghost" className="min-h-11" onClick={() => remove(id)}>
            Forget this harness
          </Button>
          <Button variant="secondary" className="min-h-11" onClick={() => setPairing(true)}>
            Pair a harness…
          </Button>
          {next !== undefined && (
            <Button variant="secondary" className="min-h-11" onClick={() => setActive(next)}>
              Switch harness
            </Button>
          )}
        </div>
      </div>
    </div>
  )
}

/**
 * Shown until the socket is open and `info()` answered (25 §4.3): the state
 * line, the pty transcript (the diagnostic when a shell rc waits for input,
 * 18 §4), and Relaunch / Quit when starting failed or the daemon went away.
 */
function LocalStartup({ onRelaunch, onQuit }: Props) {
  const state = useConnection((s) => s.state)
  const started = useConnection((s) => s.client !== null)
  const error = useConnection((s) => s.error)
  const [log, setLog] = useState('')
  const pre = useRef<HTMLPreElement>(null)

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

  useEffect(() => {
    pre.current?.scrollTo(0, pre.current.scrollHeight)
  }, [log])

  const status =
    error ?? (!started ? 'Starting…' : state === 'reconnecting' ? 'Reconnecting…' : 'Connecting…')
  const stuck = error !== undefined || state === 'reconnecting'

  return (
    <div
      className="flex h-dvh items-center justify-center bg-background text-foreground"
      data-tauri-drag-region={isMacDesktop() || undefined}
    >
      <div className="flex w-full max-w-[640px] flex-col gap-4 rounded-xl border border-border-subtle bg-elevated p-6">
        <p className={error ? 'text-danger' : 'text-fg-secondary'}>{status}</p>
        {log && (
          <pre
            ref={pre}
            className="max-h-[50vh] overflow-auto rounded-md bg-layer-2 p-3 font-mono text-xs leading-5 whitespace-pre-wrap break-all text-fg-secondary"
          >
            {log}
          </pre>
        )}
        <div className="flex justify-end gap-2">
          <Button variant="ghost" onClick={onQuit}>
            Quit
          </Button>
          {stuck && (
            <Button
              onClick={() => {
                setLog('')
                onRelaunch()
              }}
            >
              Relaunch
            </Button>
          )}
        </div>
      </div>
    </div>
  )
}
