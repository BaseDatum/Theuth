import '@/styles.css'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { App } from '@/App'
import { initTheme } from '@/lib/theme'
import { trackViewport } from '@/lib/viewport'

initTheme()
trackViewport()

const root = document.getElementById('root')
if (!root) throw new Error('#root missing')
createRoot(root).render(
  <StrictMode>
    <App />
  </StrictMode>,
)
