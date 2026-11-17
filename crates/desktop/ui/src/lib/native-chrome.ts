/**
 * Design 198 phase 1: the bridge to the first-party `glass` Tauri plugin (`crates/tauri-plugin-glass`), which floats
 * the iPhone's chat chrome as native iOS 26 Liquid Glass over the webview: the top bar (drawer · title pill · ⋯) and
 * the scroll-to-bottom button. Used only where `container() === 'ios'` and the plugin says `is_supported` (iOS 26
 * on an iPhone); everywhere else nothing here ever calls the plugin and the web chrome renders as before.
 *
 * One bar for the app: the `Header` that owns it (`mountBar`) feeds the title and handles the taps; the bar hides
 * whenever no header owns it or a web dialog, sheet or the drawer covers the chat (`covered`). Commands go out in
 * order through one promise chain, so a hide never overtakes the configure before it.
 */
import { addPluginListener, invoke } from '@tauri-apps/api/core'
import { useEffect } from 'react'
import { create } from 'zustand'
import { useIsMobile } from '@/hooks/use-mobile'
import { container } from '@/lib/platform'
import { subscribeDark } from '@/lib/theme'

/** The native bar's band under the safe-area top (`GlassOverlay.swift`: 4 + 44 + 4 pt), the web header's height. */
export const BAR = 52

/** What a tap on a native control asks for (`tap { id }`). */
export type GlassTap = 'drawer' | 'title' | 'menu' | 'scroll_bottom'

/** `supported`: the plugin answered `is_supported` true. False until it does, and for good off iOS. */
export const useNativeChrome = create<{ supported: boolean }>(() => ({ supported: false }))

let probed: Promise<boolean> | null = null

/** Ask the plugin once; a missing plugin or any failure counts as unsupported. */
export function probeNativeChrome(): Promise<boolean> {
  probed ??= (
    container() !== 'ios'
      ? Promise.resolve(false)
      : invoke<{ supported?: boolean }>('plugin:glass|is_supported').then(
          (r) => r?.supported === true,
          () => false,
        )
  ).then((supported) => {
    if (supported) useNativeChrome.setState({ supported })
    return supported
  })
  return probed
}

/** Whether the native bar stands in for a header that would carry the drawer button (`own`: the window's top-left
 * pane): supported, and the phone layout. An iPad's narrow window never gets here (`is_supported` is iPhone only). */
export function useNativeBar(own: boolean): boolean {
  const supported = useNativeChrome((s) => s.supported)
  const mobile = useIsMobile()
  useEffect(() => void probeNativeChrome(), [])
  return supported && mobile && own
}

let chain: Promise<void> = Promise.resolve()
function send(command: string, args: Record<string, unknown>): void {
  chain = chain
    .then(() => invoke(`plugin:glass|${command}`, args))
    .then(
      () => {},
      (e: unknown) => console.warn(`glass ${command}:`, e),
    )
}

/** Test hook: resolves once every command sent so far has been answered. */
export const nativeChromeSettled = (): Promise<void> => chain

/** A dialog that covers the chat: any Radix dialog (sheets, the phone drawer, centred and full-screen dialogs) but
 * not a popover (the chevron menu hangs under the bar and keeps it). */
export function covered(root: Pick<ParentNode, 'querySelectorAll'> = document): boolean {
  for (const el of root.querySelectorAll('[role="dialog"],[role="alertdialog"]')) if (!el.closest('[data-radix-popper-content-wrapper]')) return true
  return false
}

const isDark = () => document.documentElement.classList.contains('dark')

const bar = {
  owners: 0,
  hidden: true,
  covered: false,
  title: '',
  dark: false,
  scroll: { visible: false, bottom: -1 },
  stop: (): void => {},
}

function syncHidden(): void {
  const hidden = bar.owners === 0 || bar.covered
  if (hidden === bar.hidden) return
  bar.hidden = hidden
  send('set_hidden', { hidden })
}

/** Portals mount and unmount as direct children of `<body>`: watching that list (not the subtree) never wakes on
 * transcript churn. */
function watch(): () => void {
  const check = () => {
    bar.covered = covered()
    syncHidden()
  }
  const portals = typeof MutationObserver === 'undefined' ? null : new MutationObserver(check)
  portals?.observe(document.body, { childList: true })
  const theme = subscribeDark(() => {
    if (isDark() === bar.dark) return
    bar.dark = isDark()
    send('configure', { title: bar.title, dark: bar.dark })
  })
  bar.covered = covered()
  return () => {
    portals?.disconnect()
    theme()
  }
}

/** Show the native bar with `title` for as long as the returned disposer has not run. */
export function mountBar(title: string): () => void {
  if (++bar.owners === 1) bar.stop = watch()
  bar.title = title
  bar.dark = isDark()
  send('configure', { title, dark: bar.dark })
  bar.hidden = false
  syncHidden()
  return () => {
    if (--bar.owners > 0) return
    bar.stop()
    syncHidden()
  }
}

export function setBarTitle(title: string): void {
  if (title === bar.title) return
  bar.title = title
  send('set_title', { title })
}

/** The native scroll-to-bottom; `bottom` is px from the visible band's bottom to the button's bottom edge. */
export function setScrollButton(visible: boolean, bottom: number): void {
  const px = Math.max(0, Math.round(bottom))
  if (visible === bar.scroll.visible && (!visible || px === bar.scroll.bottom)) return
  bar.scroll = { visible, bottom: px }
  send('set_scroll_button', { visible, bottom: px })
}

const taps = new Set<(id: GlassTap) => void>()
let listening: Promise<unknown> | null = null

/** Calls `cb` for every tap on a native control until the disposer runs; one plugin listener serves them all. */
export function onNativeTap(cb: (id: GlassTap) => void): () => void {
  taps.add(cb)
  listening ??= addPluginListener<{ id: GlassTap }>('glass', 'tap', ({ id }) => {
    for (const t of [...taps]) t(id)
  }).catch((e: unknown) => {
    listening = null
    console.warn('glass tap listener:', e)
  })
  return () => {
    taps.delete(cb)
  }
}

/**
 * The visible band's bottom minus the scroller's, plus the web button's 12 px gap: where the native button's bottom
 * edge sits above the keyboard (or the screen's bottom). `vv` is `window.visualViewport` in layout px.
 */
export function scrollButtonBottom(scrollerBottom: number, vv: { height: number; offsetTop: number }): number {
  return vv.offsetTop + vv.height - scrollerBottom + 12
}

/** Test hook: forget the probe, the bar and the listeners. */
export function resetNativeChrome(): void {
  probed = null
  listening = null
  chain = Promise.resolve()
  taps.clear()
  bar.stop()
  Object.assign(bar, { owners: 0, hidden: true, covered: false, title: '', dark: false, scroll: { visible: false, bottom: -1 }, stop: () => {} })
  useNativeChrome.setState({ supported: false })
}
