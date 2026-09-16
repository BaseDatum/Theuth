import '@/styles.css'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { openUrl } from '@tauri-apps/plugin-opener'
import { toast } from 'sonner'
import { App } from '@/App'
import { container } from '@/lib/platform'
import { initTheme } from '@/lib/theme'
import { trackViewport } from '@/lib/viewport'

initTheme()
trackViewport()
// The webview has no popup handler, so `window.open` (Streamdown's link-safety modal, etc.) is a silent no-op; hand it to the system browser.
if (container() !== 'web') {
  window.open = (url) => {
    if (url) openUrl(String(url)).catch((err: unknown) => toast.error(String(err)))
    return null
  }
}

const root = document.getElementById('root')
if (!root) throw new Error('#root missing')
createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
