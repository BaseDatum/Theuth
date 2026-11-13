// 155 §7.6 (D6): the engine decides *whether* an attention item alerts (`AttentionItem.alert`, already blanked by
// rpc for this connection's focused sessions and seen finishes); the desktop decides *how* — one system
// notification per `alert.id`. The event-driven path (`ApprovalRequested`, root `TurnFinished`) stays for items the
// engine does not decide: it is held for a grace period and dropped once the list shows the item carrying the
// `alert` key (null or not — the engine's answer), so an old engine still notifies and a new one never twice.

import type { AttentionItem } from "@/rpc/bindings/method.roster.output"

/** How long an event-driven notification waits for the engine's decision to show up in `attention_list`. */
export const LEGACY_GRACE_MS = 3_000

/** One held event-driven notification; `live` flips off when the engine's decision or `dispose` arrives first. */
type Pending = { live: boolean }

export class AlertNotifier {
  private readonly seen = new Set<string>()
  private readonly pending = new Map<string, Pending>()

  /** `visible(session)`: the session is on screen in this window — never notify for it. */
  constructor(
    private readonly notify: (title: string, body: string) => void,
    private readonly visible: (session: string) => boolean,
    private readonly graceMs = LEGACY_GRACE_MS,
  ) {}

  /** An event that may become an alert-bearing item whose id starts with `key`; posts after the grace unless it does. */
  legacy(key: string, title: string, body: string): void {
    this.cancel(key)
    const held: Pending = { live: true }
    this.pending.set(key, held)
    setTimeout(() => {
      if (!held.live) return
      this.pending.delete(key)
      this.notify(title, body)
    }, this.graceMs)
  }

  /** The attention list arrived: settle pending legacy events, post one notification per unseen `alert.id`. */
  list(items: readonly AttentionItem[]): void {
    for (const key of this.pending.keys()) {
      if (items.some((i) => i.id.startsWith(key) && 'alert' in i)) this.cancel(key)
    }
    for (const i of items) {
      const alert = i.alert
      if (!alert || this.seen.has(alert.id)) continue
      this.seen.add(alert.id)
      if (i.session && this.visible(i.session)) continue
      this.notify(i.title, alert.why)
    }
  }

  dispose(): void {
    for (const key of [...this.pending.keys()]) this.cancel(key)
  }

  private cancel(key: string): void {
    const p = this.pending.get(key)
    if (!p) return
    p.live = false
    this.pending.delete(key)
  }
}
