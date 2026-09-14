/** Settings › General theme; `system` follows `prefers-color-scheme`. */
export type Theme = 'light' | 'dark' | 'system'

const KEY = 'theme'
const scheme = matchMedia('(prefers-color-scheme: dark)')

/** The stored choice; `system` when unset. */
export function currentTheme(): Theme {
  const t = localStorage.getItem(KEY)
  return t === 'light' || t === 'dark' ? t : 'system'
}

/** Persist `theme` and set the `.dark` class on `<html>` accordingly. */
export function setTheme(theme: Theme): void {
  localStorage.setItem(KEY, theme)
  apply()
}

function apply(): void {
  const theme = currentTheme()
  document.documentElement.classList.toggle('dark', theme === 'dark' || (theme === 'system' && scheme.matches))
}

/** Apply the stored theme now and keep following the OS while it is `system`. */
export function initTheme(): void {
  scheme.addEventListener('change', apply)
  apply()
}
