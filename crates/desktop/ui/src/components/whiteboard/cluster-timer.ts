// 218 §8.8 / design §4.2: when the control cluster is up. Raised by a finger or mouse tap on empty canvas or by the
// pill's grabber — never by a pen; hidden after 4 s without finger input, on any pen activity, or on a second tap.
// ⋯ → "Keep controls open" stops the timer (WCAG 2.2.1). Pure timer logic; the view mirrors `open` into the store.

export const HIDE_MS = 4_000

export type ClusterTimer = {
  /** A finger/mouse tap on empty canvas: raise, or hide when already up. */
  tap(): void
  /** The grabber: raise (or hide when up), same as a tap. */
  grabber(): void
  /** Raise without toggling (a Pencil "show tools" gesture, a keyboard shortcut). */
  show(): void
  /** Finger or pointer input on the cluster itself: restart the 4 s. */
  activity(): void
  /** Pen down (pointer or native): hide at once. */
  pen(): void
  hide(): void
  /** `keepControls` changed: on → no timer; off → the 4 s starts again while up. */
  keep(on: boolean): void
  /** A cluster popover is open (on) or closed (off): no auto-hide while one is open; closing restarts the 4 s. */
  hold(on: boolean): void
  isOpen(): boolean
  dispose(): void
}

export function clusterTimer(o: { onChange(open: boolean): void; keep?: boolean; hideMs?: number }): ClusterTimer {
  let open = false
  let keep = o.keep ?? false
  let held = false
  let timer: ReturnType<typeof setTimeout> | null = null
  const ms = o.hideMs ?? HIDE_MS
  const disarm = () => {
    if (timer !== null) clearTimeout(timer)
    timer = null
  }
  const arm = () => {
    disarm()
    if (open && !keep && !held) timer = setTimeout(() => set(false), ms)
  }
  const set = (next: boolean) => {
    if (next === open) {
      if (next) arm()
      return
    }
    open = next
    if (open) arm()
    else disarm()
    o.onChange(open)
  }
  const toggle = () => set(!open)
  return {
    tap: toggle,
    grabber: toggle,
    show: () => set(true),
    activity: () => open && arm(),
    pen: () => set(false),
    hide: () => set(false),
    keep: (on) => {
      keep = on
      arm()
    },
    hold: (on) => {
      held = on
      arm()
    },
    isOpen: () => open,
    dispose: disarm,
  }
}
