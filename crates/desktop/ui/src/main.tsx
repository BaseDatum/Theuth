import '@/styles.css'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { openUrl } from '@tauri-apps/plugin-opener'
import { toast } from 'sonner'
import { App } from '@/App'
import { GuestApp } from '@/components/share/GuestApp'
import { isGuest } from '@/link/guest'
import { container } from '@/lib/platform'
import { initTheme } from '@/lib/theme'
import { syncPageBackground } from '@/lib/native-chrome'
import { trackViewport } from '@/lib/viewport'

initTheme()
trackViewport()
// 198: iOS's native layers behind the webview follow the page's theme (the band under the composer).
syncPageBackground()
// The webview has no popup handler, so `window.open` (Streamdown's link-safety modal, etc.) is a silent no-op; hand it to the system browser.
if (container() !== 'web') {
  window.open = (url) => {
    if (url) openUrl(String(url)).catch((err: unknown) => toast.error(String(err)))
    return null
  }
}

const root = document.getElementById('root')
if (!root) throw new Error('#root missing')
// 59 §8.2: the relay serves this bundle at `/d` too; a share link boots the guest container instead of the app.
createRoot(root).render(<StrictMode>{isGuest() ? <GuestApp /> : <App />}</StrictMode>)
