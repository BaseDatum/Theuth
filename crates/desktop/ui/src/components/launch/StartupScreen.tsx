import { useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { launchLog, onLaunchLog } from '@/lib/tauri'
import { useConnection } from '@/store/connection'

type Props = {
  onRelaunch: () => void
  onQuit: () => void
}

/**
 * Shown until the socket is open and `info()` answered (25 §4.3): the state
 * line, the pty transcript (the diagnostic when a shell rc waits for input,
 * 18 §4), and Relaunch / Quit when starting failed or the daemon went away.
 */
export function StartupScreen({ onRelaunch, onQuit }: Props) {
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

  const status = error ?? (!started ? 'Starting…' : state === 'reconnecting' ? 'Reconnecting…' : 'Connecting…')
  const stuck = error !== undefined || state === 'reconnecting'

  return (
    <div className="flex h-screen items-center justify-center bg-background text-foreground" data-tauri-drag-region>
      <div className="flex w-[640px] max-w-[90vw] flex-col gap-4 rounded-xl border border-border-subtle bg-elevated p-6">
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
