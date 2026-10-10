/**
 * The engine's refusal for an id it does not hold (`crates/agent` store/supervisor), bare or with the id prefixed by
 * `crates/rpc` (`"<id>: no such session"`). For the route's own session it is not an error to show: the chat was
 * deleted, the deep link is stale, or the notification named a session on another engine.
 */
export const isNoSuchSession = (e: unknown): boolean => {
  const message = e instanceof Error ? e.message : typeof e === 'string' ? e : null
  return message !== null && /(^|: )no such session$/.test(message)
}
