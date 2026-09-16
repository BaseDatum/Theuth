import { Format, cancel, checkPermissions, requestPermissions, scan } from '@tauri-apps/plugin-barcode-scanner'
import { Loader2, QrCode } from 'lucide-react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Textarea } from '@/components/ui/textarea'
import { container, deviceName } from '@/lib/platform'
import { useHarnesses } from '@/link/harnesses'
import { pair, parsePairingKey } from '@/link/pairing'

/** Read and strip a `#1.<…>` pairing hash from the address bar: the secret should not outlive this mount in history. */
function takeHashKey(): string | null {
  if (!location.hash.startsWith('#1.')) return null
  const key = location.href
  history.replaceState(history.state, '', location.pathname + location.search)
  return key
}

/**
 * 31 §7.5 device side: "Scan QR" (iOS) and "Paste key", both through `pair()`.
 * Rendered by `/link` and, when nothing is paired, by `App` outside the router —
 * so navigation is the caller's (`onPaired`); the harness switch itself routes to `/`.
 */
export function PairScreen({ onPaired }: { onPaired?: () => void }) {
  const [key, setKey] = useState('')
  const [pairingWith, setPairingWith] = useState<string | null>(null)
  const [scanning, setScanning] = useState(false)
  const ios = container() === 'ios'

  const start = async (raw: string) => {
    const parsed = parsePairingKey(raw.trim())
    if (!parsed) {
      toast.error('not a pairing key')
      return
    }
    setPairingWith(parsed.name)
    try {
      const h = await pair(raw.trim(), deviceName())
      toast.success(`Paired ${h.name}`)
      useHarnesses.getState().setActive(h.id)
      onPaired?.()
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setPairingWith(null)
    }
  }

  // Relay-served landing / universal link: `/link#1.…` pairs on arrival.
  useEffect(() => {
    const auto = takeHashKey()
    if (auto) void start(auto)
  }, [])

  // The camera preview sits behind the webview: everything from `<body>` down goes transparent while scanning.
  useEffect(() => {
    if (!scanning) return
    const root = document.getElementById('pair-screen')
    const saved: [HTMLElement, string][] = []
    for (let el: HTMLElement | null = root; el; el = el.parentElement) {
      saved.push([el, el.style.background])
      el.style.background = 'transparent'
    }
    return () => {
      for (const [el, bg] of saved) el.style.background = bg
    }
  }, [scanning])

  const scanQr = async () => {
    let perm = await checkPermissions()
    if (perm !== 'granted') perm = await requestPermissions()
    if (perm !== 'granted') {
      toast.error('Camera access is off for this app')
      return
    }
    setScanning(true)
    try {
      const { content } = await scan({ windowed: true, formats: [Format.QRCode] })
      setScanning(false)
      await start(content)
    } catch {
      // cancelled (or the plugin failed): back to the form
      setScanning(false)
    }
  }

  const cancelScan = async () => {
    await cancel()
    setScanning(false)
  }

  if (scanning) {
    return (
      <div id="pair-screen" className="flex min-h-dvh flex-col items-center justify-end p-6 pb-[max(1.5rem,env(safe-area-inset-bottom))]">
        <Button variant="secondary" className="min-h-11 w-full max-w-sm" onClick={() => void cancelScan()}>
          Cancel
        </Button>
      </div>
    )
  }

  return (
    <div
      id="pair-screen"
      className="flex min-h-dvh flex-col items-center justify-center gap-6 px-6 pt-[max(1.5rem,env(safe-area-inset-top))] pb-[max(1.5rem,env(safe-area-inset-bottom))]"
    >
      <div className="flex w-full max-w-sm flex-col gap-4">
        <h1 className="text-2xl font-semibold">Connect to a harness</h1>
        {pairingWith !== null ? (
          <p className="flex items-center gap-2 text-fg-secondary">
            <Loader2 className="size-4 animate-spin" /> Pairing with {pairingWith}…
          </p>
        ) : (
          <>
            {ios && (
              <Button className="min-h-11 w-full" onClick={() => void scanQr()}>
                <QrCode className="size-5" /> Scan QR
              </Button>
            )}
            <label className="flex flex-col gap-2">
              <span className="text-sm text-fg-secondary">Paste key</span>
              <Textarea
                value={key}
                onChange={(e) => setKey(e.target.value)}
                placeholder="https://relay…/link#1.…"
                rows={3}
                autoCapitalize="off"
                autoCorrect="off"
                spellCheck={false}
                className="font-mono text-xs"
              />
            </label>
            <Button variant={ios ? 'secondary' : 'default'} className="min-h-11 w-full" disabled={key.trim() === ''} onClick={() => void start(key)}>
              Pair
            </Button>
          </>
        )}
      </div>
      <p className="max-w-sm text-center text-xs text-fg-tertiary">
        The relay carries encrypted traffic between your devices and this harness. It can see that they are connected, not what they say.
      </p>
    </div>
  )
}
