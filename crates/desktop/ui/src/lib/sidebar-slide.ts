/** Dispatched on `window` as the desktop sidebar's open/close slide starts (`Sidebar`, a Web Animation, which fires no
 * DOM event of its own), so `followRect` (a native web view laid over the column) follows the column every frame. */
export const SIDEBAR_SLIDE_EVENT = 'theuth:sidebar-slide'
