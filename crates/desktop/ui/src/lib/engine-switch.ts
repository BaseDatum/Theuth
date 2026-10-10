// The gate between `useEngines.active` and the engine whose Shell is on screen (48 §5). A switch unmounts the
// engine left at once, navigates to the destination engine's last chat (else `/`), and lets the new Shell mount
// only once the router holds that location: a `/c/<session>` of the engine left never renders under the new
// engine's provider, so none of its session RPCs reach an engine that answers "no such session".

type Location = { pathname: string; href: string }

/** `navigate` resolves once the router holds `href` (matches committed), even with no RouterProvider mounted. */
export class EngineSwitch {
  /** The engine whose Shell is mounted; `null` while a switch is in flight (before the first `activate` nothing gates). */
  shown: string | null = null
  #started = false
  #token = 0
  /** The chat each engine was showing when the user switched away, restored on the way back. */
  readonly lastChat = new Map<string, string>()
  readonly #listeners = new Set<() => void>()

  constructor(private readonly navigate: (href: string) => Promise<void>) {}

  /** `useSyncExternalStore` seam. */
  subscribe = (fn: () => void): (() => void) => {
    this.#listeners.add(fn)
    return () => void this.#listeners.delete(fn)
  }

  /** The engine whose Shell may render now: `active`, or `null` while a switch is in flight. */
  visible(active: string): string | null {
    if (!this.#started) return active
    return this.shown === active ? active : null
  }

  /**
   * `active` changed (or hydration read it). The first call shows at once, deep link kept. A later one records
   * the chat left (`loc` names it only while that engine's tree is on screen), navigates to the destination's last
   * chat, and shows it when the navigation has landed — unless a newer switch superseded it. Resolves when settled.
   */
  activate(active: string, loc: Location): Promise<void> {
    if (!this.#started) {
      this.#started = true
      this.#show(active)
      return Promise.resolve()
    }
    if (this.shown === active) return Promise.resolve()
    if (this.shown !== null) {
      if (loc.pathname.startsWith('/c/')) this.lastChat.set(this.shown, loc.href)
      else this.lastChat.delete(this.shown)
      this.#show(null)
    }
    const token = ++this.#token
    const land = () => {
      if (token === this.#token) this.#show(active)
    }
    return this.navigate(this.lastChat.get(active) ?? '/').then(land, land)
  }

  #show(engine: string | null): void {
    if (this.shown === engine) return
    this.shown = engine
    for (const fn of this.#listeners) fn()
  }
}
