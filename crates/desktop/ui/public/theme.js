// Same rule as src/lib/theme.ts, run blocking in <head> before first paint so dark mode never flashes bone.
// External (not inline) because both the Tauri and relay CSPs are `script-src 'self'`.
var t = localStorage.getItem('theme')
if (t === 'dark' || (t !== 'light' && matchMedia('(prefers-color-scheme: dark)').matches))
  document.documentElement.classList.add('dark')
