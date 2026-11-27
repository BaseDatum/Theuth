import { useEffect, useState } from 'react'
import { Button } from '@/components/ui/button'
import { useConnection } from '@/store/connection'
import { isUnsupported, vaultApi } from './api'
import { PasswordUnlockSheet } from './PasswordUnlockSheet'

/** How often the connected engine's vault state is read (no event carries it, 225 §3). */
export const SEALED_POLL_MS = 10_000

/**
 * 225 × 224 §16.12: the connected engine itself is sealed — a cloud node that restarted, reached from a phone that is not
 * a member of its mesh, so no device of the mesh can unlock it. One card with *Unlock*, which asks for the master
 * password (`vault_unseal(null, pw)` on that engine, end-to-end over the link). Nothing when the engine has no vault
 * methods or is unsealed.
 */
export function SealedEngineCard({ floating = false }: { floating?: boolean }) {
  const client = useConnection((s) => s.client)
  const discovery = useConnection((s) => s.discovery)
  const name = useConnection((s) => s.info?.name) ?? 'This engine'
  const [sealed, setSealed] = useState(false)
  const [open, setOpen] = useState(false)

  useEffect(() => {
    setSealed(false)
    if (!client) return
    const api = vaultApi(client)
    let stopped = false
    let timer: ReturnType<typeof setTimeout> | undefined
    const poll = async () => {
      try {
        const s = await api.status(null)
        if (stopped) return
        setSealed(s.state === 'sealed')
      } catch (e) {
        if (isUnsupported(e)) return
      }
      if (!stopped) timer = setTimeout(() => void poll(), SEALED_POLL_MS)
    }
    void poll()
    return () => {
      stopped = true
      clearTimeout(timer)
    }
  }, [client, discovery, open])

  if (!sealed) return null
  return (
    <div
      role="alert"
      data-sealed-engine=""
      className={`flex items-center gap-3 rounded-lg border border-border bg-background p-3 text-sm shadow-lg ${floating ? 'fixed top-[calc(var(--safe-top)+3rem)] right-4 z-50 w-96 max-w-[calc(100vw-2rem)]' : ''}`}
    >
      <span className="min-w-0 flex-1">{name}'s vault is locked. Unlock it with your master password.</span>
      <Button size="sm" onClick={() => setOpen(true)}>
        Unlock
      </Button>
      <PasswordUnlockSheet engine={null} name={name} open={open} onOpenChange={setOpen} />
    </div>
  )
}
