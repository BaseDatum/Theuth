import { createFileRoute } from '@tanstack/react-router'
import { useEffect, useState } from 'react'
import { WebFrame } from '@/components/chat/WebFrame'
import { useWebViews } from '@/lib/web-views'

type Search = { url: string; title: string }

/** One web view frame, full window (`lib/popout.ts` kind `web`, 205 D6): the window hosts the shell's `web-<id>` child
 * web view, which reloads at `url`. Under the `Popped` root when opened with `?window=`. */
export const Route = createFileRoute('/web/$id')({
  // The router reads search values as JSON where they parse (`title=42` is a number): strings either way.
  validateSearch: (s: Record<string, unknown>): Search => ({
    url: s.url === undefined ? '' : String(s.url),
    title: s.title === undefined ? '' : String(s.title),
  }),
  component: WebRoute,
})

function WebRoute() {
  const { id } = Route.useParams()
  const { url, title } = Route.useSearch()
  // This window has its own store: the frame's entry comes from the URL, once.
  useState(() => useWebViews.getState().views[id] ?? useWebViews.getState().open({ id, session: '', url, title: title || url }))
  const shown = useWebViews((s) => s.views[id]?.title)
  useEffect(() => {
    if (shown) document.title = shown
  }, [shown])
  return <WebFrame id={id} popped />
}
