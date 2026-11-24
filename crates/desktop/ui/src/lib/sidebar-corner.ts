/** The sidebar state a header reads (`useSidebar()`). */
type Corner = { open: boolean; isMobile: boolean; rail: boolean }

/** A pane or page header at the window's left edge carries the sidebar toggle: on a phone (the Sheet's only opener)
 * and while a desktop sidebar without a rail is hidden. The rail has its own toggle. */
export const headerToggle = (s: Corner): boolean => s.isMobile || (!s.open && !s.rail)

/** The left padding of a header at the window's top-left corner on macOS, clear of the traffic lights: the whole 5rem
 * while nothing covers that corner, the part the lights overhang the 3.5rem rail while it does, none when expanded. */
export const cornerPad = (s: Corner): string | undefined => (s.open ? undefined : s.rail ? 'pl-6' : 'pl-20')
