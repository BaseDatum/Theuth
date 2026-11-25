import { Copy, Eye, EyeOff } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { errorText } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { notify } from '@/lib/notify'
import { copySecret } from '@/lib/secret-clipboard'
import { useApi } from '@/lib/rpc'

/** D25: a revealed value is dropped this long after it arrives. */
export const REVEAL_MS = 30_000

/**
 * D25 Reveal for one value-bearing ref (`aws/prod/db#password`, `bw/personal/<id>#username`). Absent when the
 * connection's `allow_reveal` is off. The press calls `reveal_external_secret` directly, not through TanStack Query: the
 * value lives in this component's state only (no query cache, no mutation cache, no storage), shows masked with an eye
 * toggle, and is dropped after `REVEAL_MS`, on Hide, on unmount, and when `allowed` turns off. Copy writes it to the
 * clipboard through `copySecret` (cleared after `CLIPBOARD_MS` while still there).
 */
export function Reveal({ conn, refText, label, allowed }: { conn: string; refText: string; label: string; allowed: boolean }) {
  const api = useApi()
  const [value, setValue] = useState<string | null>(null)
  const [shown, setShown] = useState(false)
  const [busy, setBusy] = useState(false)
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const hide = () => {
    clearTimeout(timer.current)
    setValue(null)
    setShown(false)
  }
  useEffect(() => () => clearTimeout(timer.current), [])
  // `allow_reveal` switched off while a value shows: drop it.
  useEffect(() => {
    if (allowed) return
    clearTimeout(timer.current)
    setValue(null)
    setShown(false)
  }, [allowed])
  if (!allowed) return null
  const reveal = async () => {
    setBusy(true)
    try {
      const v = await api.reveal_external_secret(conn, refText)
      setValue(v)
      clearTimeout(timer.current)
      timer.current = setTimeout(hide, REVEAL_MS)
    } catch (e) {
      notify.error(errorText(e))
    } finally {
      setBusy(false)
    }
  }
  const copy = async () => {
    if (value === null) return
    await copySecret(value)
    notify.success('Copied; the clipboard clears in 60 s unless you copy something else')
  }
  if (value === null)
    return (
      <Button variant="ghost" size="xs" disabled={busy} aria-label={`Reveal ${label}`} onClick={() => void reveal()}>
        Reveal {label}
      </Button>
    )
  return (
    <span className="flex items-center gap-1">
      <Input readOnly type={shown ? 'text' : 'password'} value={value} aria-label={`value of ${label}`} autoComplete="off" className="h-7 w-56 font-mono text-xs" />
      <Button variant="ghost" size="icon-sm" aria-label={shown ? `Mask ${label}` : `Show ${label}`} onClick={() => setShown(!shown)}>
        {shown ? <EyeOff /> : <Eye />}
      </Button>
      <Button variant="ghost" size="icon-sm" aria-label={`Copy ${label}`} onClick={() => void copy().catch((e) => notify.error(errorText(e)))}>
        <Copy />
      </Button>
      <Button variant="ghost" size="xs" onClick={hide}>
        Hide
      </Button>
    </span>
  )
}
