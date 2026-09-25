import { Format, cancel, checkPermissions, requestPermissions, scan } from '@tauri-apps/plugin-barcode-scanner'
import { ChevronLeft, Loader2, QrCode } from 'lucide-react'
import { useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Textarea } from '@/components/ui/textarea'
import { container, deviceName } from '@/lib/platform'
import { LOCAL, useHarnesses } from '@/link/harnesses'
import { adopt } from '@/link/nodes'
import { pair, parsePairingKey } from '@/link/pairing'
import { useConnection } from '@/store/connection'

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
 * With an active harness that answers `list_nodes`, a key first asks: pair here, or
 * add the harness as a node of the active one and auto-pair with it (32 §8.3).
 */
export function PairScreen({ onPaired, onBack }: { onPaired?: () => void; onBack?: () => void }) {
  const [key, setKey] = useState('')
  const [pairingWith, setPairingWith] = useState<string | null>(null)
  const [scanning, setScanning] = useState(false)
  const [choice, setChoice] = useState<{ key: string; name: string } | null>(null)
  const ios = container() === 'ios'
  const parent = useHarnesses((s) => (s.nodes[s.active] ? s.active : null))
  const infoName = useConnection((s) => s.info?.name)
  const storedName = useHarnesses((s) => s.harnesses.find((h) => h.id === s.active)?.name)
  const parentName = infoName ?? storedName ?? (parent === LOCAL ? 'this Mac' : 'the active harness')
  const api = useConnection((s) => s.api)

  const start = async (raw: string) => {
    const parsed = parsePairingKey(raw.trim())
    if (!parsed) {
      toast.error('not a pairing key')
      return
    }
    // A mesh key (4 parts, 69 §5) is this device's own pairing; `add_node` takes engine keys only.
    if (parent && api && parsed.pending === undefined) setChoice({ key: raw.trim(), name: parsed.name })
    else await pairHere(raw.trim(), parsed.name)
  }

  const pairHere = async (k: string, name: string) => {
    setPairingWith(name)
    try {
      const h = await pair(k, deviceName())
      toast.success(h.kind === 'mesh' ? `Paired with mesh ${h.meshName ?? h.name}` : `Paired ${h.name}`)
      useHarnesses.getState().setActive(h.id)
      onPaired?.()
    } catch (e) {
      toast.error(e instanceof Error ? e.message : String(e))
    } finally {
      setPairingWith(null)
    }
  }

  // `add_node` on the parent, then this device pairs with the node as it would on first sight (32 §8.2).
  const asNode = async (k: string, name: string) => {
    if (!parent || !api) return
    setPairingWith(name)
    try {
      const node = await api.add_node(k, null)
      await adopt(parent, api, node)
      toast.success(`Added ${node.name} to ${parentName}`)
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
      <div id="pair-screen" className="flex h-band flex-col items-center justify-end p-6 pb-[max(1.5rem,var(--safe-bottom))]">
        <Button variant="secondary" className="min-h-11 w-full max-w-sm" onClick={() => void cancelScan()}>
          Cancel
        </Button>
      </div>
    )
  }

  return (
    <div
      id="pair-screen"
      className="flex h-band flex-col items-center gap-6 overflow-y-auto px-6 pt-[max(1.5rem,var(--safe-top))] pb-[max(1.5rem,var(--safe-bottom))]"
    >
      {/* Auto margins, not `justify-center`: they centre without clipping the top when the keyboard shrinks the band. */}
      <div className="mt-auto flex w-full max-w-sm flex-col gap-4">
        {onBack && (
          <Button variant="ghost" className="h-11 justify-start self-start px-2 text-fg-secondary" onClick={onBack}>
            <ChevronLeft className="size-5" /> Back
          </Button>
        )}
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
      <p className="mb-auto max-w-sm text-center text-xs text-fg-tertiary">
        The relay carries encrypted traffic between your devices and this harness. It can see that they are connected, not what they say.
      </p>
      <Dialog open={choice !== null} onOpenChange={(o) => !o && setChoice(null)}>
        <DialogContent className="max-w-sm">
          <DialogHeader>
            <DialogTitle>Pair {choice?.name}</DialogTitle>
            <DialogDescription>A node runs sessions for {parentName}; this device connects to it too.</DialogDescription>
          </DialogHeader>
          <div className="flex flex-col gap-2">
            <Button
              className="min-h-11 w-full"
              onClick={() => {
                const c = choice
                setChoice(null)
                if (c) void asNode(c.key, c.name)
              }}
            >
              as a node of {parentName}
            </Button>
            <Button
              variant="secondary"
              className="min-h-11 w-full"
              onClick={() => {
                const c = choice
                setChoice(null)
                if (c) void pairHere(c.key, c.name)
              }}
            >
              as a harness on this {ios ? 'phone' : 'device'}
            </Button>
          </div>
        </DialogContent>
      </Dialog>
    </div>
  )
}
