import './openui-devtools-off'
/* OpenUI's typography (design 46): Inter with the full variable `wght` axis, bundled — no runtime font fetch; the
   app renderer is set in it through `font-variation-settings` (`components/apps/openui-theme.tsx`). */
import '@fontsource-variable/inter'
import '@/styles.css'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { openUrl } from '@tauri-apps/plugin-opener'
import { App } from '@/App'
import { GuestApp } from '@/components/share/GuestApp'
import { isGuest } from '@/link/guest'
import { notify } from '@/lib/notify'
import { container } from '@/lib/platform'
import { initTheme } from '@/lib/theme'
import { trackViewport } from '@/lib/viewport'

initTheme()
trackViewport()
// The webview has no popup handler, so `window.open` (Streamdown's link-safety modal, etc.) is a silent no-op; hand it to the system browser.
if (container() !== 'web') {
  window.open = (url) => {
    if (url) openUrl(String(url)).catch((err: unknown) => notify.error(String(err)))
    return null
  }
}

const root = document.getElementById('root')
if (!root) throw new Error('#root missing')
// 59 §8.2: the relay serves this bundle at `/d` too; a share link boots the guest container instead of the app.
createRoot(root).render(<StrictMode>{isGuest() ? <GuestApp /> : <App />}</StrictMode>)
