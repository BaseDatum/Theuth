import { RotateCcw, WifiOff } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { Terminal } from '@/components/chat/TerminalViewer'
import { Button } from '@/components/ui/button'
import { useEngineLabel } from '@/lib/owner'
import { closePopout, popoutLabel, registerNodeTerminalPopout } from '@/lib/popout'
import { useApi } from '@/lib/rpc'
import { isTauri } from '@/lib/tauri'
import type { Requirements } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'

export const nodeTerminalRequirements = {
  methods: ['engine_shell', 'shell_close', 'shell_input', 'shell_resize', 'subscribe_shell', 'unsubscribe_shell'],
  topics: [],
} as const satisfies Requirements

/**
 * `/terminal/$engine/$shell`: a standalone login shell on a mesh/cloud node in its own OS window
 * (no session, no job, no sidebar, no composer). Closing the window ends the remote shell
 * (`shell_close`, SIGHUPing its process group). A mid-stream drop stays inside this window as a
 * disconnected state with a Reconnect action.
 */
export function NodeTerminal({ engine, shell, name }: { engine: string; shell: string; name?: string }) {
  const api = useApi()
  const connState = useConnection((s) => s.state)
  const engineLabel = useEngineLabel(engine)
  const title = name?.trim() || engineLabel || engine

  const [attempt, setAttempt] = useState(0)
  const [exited, setExited] = useState(false)
  const [disconnected, setDisconnected] = useState<string | null>(null)
  const [reconnecting, setReconnecting] = useState(false)
  const closedRef = useRef(false)

  useEffect(() => {
    document.title = title
  }, [title])

  // Record this window for the main window's `popout-closed` handler (Tauri desktop), and also
  // close the remote shell on `pagehide`/`beforeunload` when running in a browser tab/window.
  useEffect(() => {
    closedRef.current = false
    const label = popoutLabel({ kind: 'node-terminal', engine, shell })
    registerNodeTerminalPopout(label, { engine, shell })
    if (isTauri()) return
    const end = () => {
      if (closedRef.current) return
      closedRef.current = true
      void api.shell_close(shell, engine).catch(() => undefined)
    }
    window.addEventListener('pagehide', end)
    window.addEventListener('beforeunload', end)
    return () => {
      window.removeEventListener('pagehide', end)
      window.removeEventListener('beforeunload', end)
    }
  }, [api, engine, shell])

  const reconnect = async () => {
    setReconnecting(true)
    try {
      await api.engine_shell(engine, shell)
      setDisconnected(null)
      setExited(false)
      setAttempt((n) => n + 1)
    } catch (e: unknown) {
      setDisconnected(e instanceof Error ? e.message : String(e))
    } finally {
      setReconnecting(false)
    }
  }

  const connLost = connState !== 'open'
  const banner = disconnected ?? (connLost ? 'Disconnected from engine' : null)

  return (
    <div className="flex h-full min-h-0 flex-1 flex-col bg-[#12100e]">
      {banner && (
        <div role="alert" className="flex shrink-0 items-center gap-2 border-b border-danger-border/40 bg-danger-bg px-3 py-2 text-xs text-danger">
          <WifiOff className="size-3.5 shrink-0" />
          <span className="min-w-0 flex-1 truncate">Disconnected: {banner}</span>
          <Button size="xs" variant="outline" disabled={reconnecting || connLost} onClick={() => void reconnect()}>
            <RotateCcw className={reconnecting ? 'animate-spin' : undefined} />
            {reconnecting ? 'Reconnecting…' : 'Reconnect'}
          </Button>
        </div>
      )}
      {!banner && exited && (
        <div role="status" className="flex shrink-0 items-center gap-2 border-b border-border/50 bg-elevated px-3 py-1.5 text-xs text-fg-secondary">
          <span className="min-w-0 flex-1 truncate">Shell exited.</span>
          <Button size="xs" variant="outline" disabled={reconnecting} onClick={() => void reconnect()}>
            <RotateCcw className={reconnecting ? 'animate-spin' : undefined} />
            Restart
          </Button>
          <Button size="xs" variant="ghost" onClick={closePopout}>
            Close
          </Button>
        </div>
      )}
      <div className="flex min-h-0 flex-1 flex-col gap-1 p-1">
        <Terminal
          key={attempt}
          source={{ kind: 'shell', shell, engine }}
          label={title}
          popped
          onExit={() => setExited(true)}
          onDisconnect={(reason) => setDisconnected(reason)}
          className="rounded-none"
        />
      </div>
    </div>
  )
}
