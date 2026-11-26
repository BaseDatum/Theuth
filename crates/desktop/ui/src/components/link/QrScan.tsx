import { Format, cancel, checkPermissions, openAppSettings, requestPermissions, scan } from '@tauri-apps/plugin-barcode-scanner'
import { useCallback, useEffect, useRef, useState } from 'react'
import { Button } from '@/components/ui/button'
import { notify as toast } from '@/lib/notify'
import { container } from '@/lib/platform'
import { cn } from '@/lib/utils'

/** The barcode scanner plugin runs in the iOS shell (iPhone and iPad); desktops and the browser have no camera path. */
export const scannerAvailable = (): boolean => container() === 'ios'

export type QrScan = {
  /** The camera is up: render `ScanningView` (only `.qr-scan-ui` stays painted). */
  scanning: boolean
  /** The last scan found the camera permission refused: render `CameraOff`. */
  denied: boolean
  /** Ask for the camera if needed, then scan one QR code: its text, or `null` (refused, cancelled or failed). */
  scan: () => Promise<string | null>
  cancel: () => Promise<void>
}

/**
 * One windowed QR scan (pairing, contacts). The camera preview sits behind the webview, so while scanning
 * `<html data-qr-scanning>` makes the page transparent and hides all but `.qr-scan-ui` (styles.css); the attribute
 * goes when the scan ends, is cancelled, or the caller unmounts, and an unmount mid-scan cancels the plugin's scan.
 */
export function useQrScan(): QrScan {
  const [scanning, setScanning] = useState(false)
  const [denied, setDenied] = useState(false)
  const live = useRef(false)

  useEffect(() => {
    if (!scanning) return
    const html = document.documentElement
    html.dataset.qrScanning = ''
    return () => void delete html.dataset.qrScanning
  }, [scanning])
  useEffect(
    () => () => {
      if (live.current) void cancel().catch(() => undefined)
    },
    [],
  )

  const run = useCallback(async (): Promise<string | null> => {
    try {
      let perm = await checkPermissions()
      if (perm !== 'granted') perm = await requestPermissions()
      setDenied(perm !== 'granted')
      if (perm !== 'granted') return null
    } catch (e) {
      toast.error(`Camera unavailable: ${e instanceof Error ? e.message : String(e)}`)
      return null
    }
    live.current = true
    setScanning(true)
    try {
      return (await scan({ windowed: true, formats: [Format.QRCode] })).content
    } catch {
      // Cancelled, or the plugin failed: back to the form.
      return null
    } finally {
      live.current = false
      setScanning(false)
    }
  }, [])
  const stop = useCallback(async () => {
    await cancel().catch(() => undefined)
    setScanning(false)
  }, [])
  return { scanning, denied, scan: run, cancel: stop }
}

/** What stays painted over the camera preview: an optional hint and Cancel, at the bottom. */
export function ScanningView({ hint, onCancel, className }: { hint?: string; onCancel: () => void; className?: string }) {
  return (
    <div className={cn('qr-scan-ui flex flex-col items-center justify-end gap-3', className)}>
      {hint && <p className="rounded-full bg-black/60 px-3 py-1.5 text-center text-sm text-white">{hint}</p>}
      <Button variant="secondary" className="min-h-11 w-full max-w-sm" onClick={onCancel}>
        Cancel
      </Button>
    </div>
  )
}

/** The camera permission was refused: iOS asks only once, after that it is the Settings app's switch. */
export function CameraOff() {
  return (
    <p className="text-[13px] text-fg-secondary">
      Camera access is off for theuth.{' '}
      <Button variant="link" className="h-auto p-0 text-[13px]" onClick={() => void openAppSettings().catch(() => undefined)}>
        Open Settings
      </Button>{' '}
      and turn on Camera to scan.
    </p>
  )
}
