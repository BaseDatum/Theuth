// Where a `show_web` frame lands (205 §4.4, 209 D11 / §7): a tile beside its chat, full screen on a phone, or the pill
// above the composer. Apart from `web-views.ts` because it places tiles (`lib/panes` imports `web-views`).

import { isMobileLayout } from '@/hooks/use-mobile'
import { useEngines } from '@/link/engines'
import { useDrafts } from '@/lib/drafts'
import { isLocal } from '@/lib/files'
import { leaves, placeWeb, usePanes } from '@/lib/panes'
import * as native from '@/lib/web-view-native'
import { arrival, isLoopback, isWeb, useWebViews, webId, webLeaf } from '@/lib/web-views'

/** `url` names the machine of a remote engine (`active` is not this desktop's own engine): this device cannot load it
 * (205 §4.4, last bullet). The frame offers **Open in browser** and **Load anyway** instead. */
export const remoteLoopback = (active: string, url: string): boolean => !isLocal(active) && isLoopback(url)

/** 209 §7: the user is writing: focus is in a composer (`data-composer`), or `session`'s composer holds a draft (text
 * or a held chip). */
export function writing(session: string): boolean {
  const active = document.activeElement
  if (active instanceof Element && active.closest('[data-composer]') !== null) return true
  const draft = useDrafts.getState().by[session]
  return draft !== undefined && (draft.text.trim() !== '' || draft.pastes.size > 0)
}

/**
 * `WebViewOpened`: the frame is recorded for its card. On a tablet or desktop an open one (tile or pop-out) navigates,
 * else a tile opens right of the session's pane when that chat is on screen (`visible`); a view that is gone (its
 * pop-out window closed) answers `no web view …`: then it is no longer popped and the tile rule applies. On a phone it
 * opens full screen, or arrives as the pill while the user is writing or looks at another chat (`arrival`); a view
 * shown before navigates in place, keeping its history.
 */
export function webViewOpened(e: { id: string; session: string; url: string; title: string }, visible: boolean): void {
  const store = useWebViews.getState()
  const old = store.views[e.id]
  store.open({ id: e.id, session: e.session, url: e.url, title: e.title })
  const phone = isMobileLayout()
  const remote = remoteLoopback(useEngines.getState().active, e.url)
  const how = arrival({ phone, native: native.nativeWebViews(), visible, writing: phone && writing(e.session), remote })
  if (phone) {
    if (how === 'card') return
    // A view never attached answers `no web view`: its attach loads the new address.
    if (old) native.navigate(e.id, e.url).catch((err: unknown) => String(err).startsWith('no web view') || console.warn('web_view_navigate', err))
    if (how === 'full') useWebViews.getState().show(e.id)
    else if (useWebViews.getState().front !== e.id) useWebViews.getState().minimise(e.id)
    return
  }
  const place = () => {
    if (how === 'tile') placeWeb(e.session, e.id)
  }
  const { root } = usePanes.getState()
  if (!old?.popped && !(root !== null && leaves(root).includes(webLeaf(e.id)))) return place()
  native.navigate(e.id, e.url).catch((err: unknown) => {
    if (!String(err).startsWith('no web view')) return console.warn('web_view_navigate', err)
    useWebViews.getState().patch(e.id, { popped: false })
    place()
  })
}

/**
 * 209 §7: the layout crossed into a phone (`phone`) or out of it (a rotation past 768 px, a wider window). Into it:
 * each web tile of the tree goes into the pill (its leaf stays, so widening again shows the tile). Out of it: the page
 * shown full screen becomes its tile beside its chat when that chat is the route's (`route`) or in the tree; else it
 * is left as it is, not minimised (no pill shows off a phone), and its card places it.
 */
export function crossLayout(phone: boolean, route: string): void {
  const web = useWebViews.getState()
  const { root } = usePanes.getState()
  if (phone) {
    for (const leaf of root ? leaves(root) : []) if (isWeb(leaf) && web.front !== webId(leaf)) web.minimise(webId(leaf))
    return
  }
  const front = web.front
  if (front === null) return
  const view = web.views[front]
  useWebViews.setState({ front: null })
  if (!view) return
  const here = view.session === route || (root !== null && leaves(root).includes(view.session))
  if (here) placeWeb(view.session, front)
}
