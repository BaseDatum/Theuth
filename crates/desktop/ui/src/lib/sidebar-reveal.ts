import { create } from 'zustand'

/**
 * The collapsed rail's "Show all N →": the section the expanded sidebar scrolls to once it mounts — a `SessionList` row
 * key (`h:active`, `h:pinned`, `p:<project>`, `p:` for the ungrouped chats) or `desks`. Taken (cleared) by whoever scrolls.
 */
export const useSidebarReveal = create<{ key: string | null }>(() => ({ key: null }))

export const revealSection = (key: string): void => useSidebarReveal.setState({ key })
export const takeReveal = (): void => useSidebarReveal.setState({ key: null })
