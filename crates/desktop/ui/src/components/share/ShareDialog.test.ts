// 184 §12.2: the Share copy by case. `shareCopy` is the rule; one markup test proves the dialog uses it (and that a lone engine reads as before).
import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { EngineProvider } from '@/lib/doc/provider'
import { ShareDialog, shareCopy } from './ShareDialog'

const mesh = { self_id: 'S', self_name: 'studio-mac', peers: [{ id: 'X', name: 'xeus', reachable: true, last_seen_ms: 0 }] as never }
let current: typeof mesh = mesh
vi.mock('@/components/tasks/queries', () => ({ useMesh: () => ({ data: current }) }))
vi.mock('@/lib/rpc', () => ({ useApi: () => ({ doc_members: async () => ({ members: [], links: [] }) }) }))
vi.mock('@/components/editor/DocumentPage', () => ({ Action: () => null }))
vi.mock('@/components/ui/dialog', () => {
  const box = (tag: string) => ({ children }: { children?: ReactNode }) => createElement(tag, null, children)
  return { Dialog: box('div'), DialogContent: box('div'), DialogHeader: box('div'), DialogTitle: box('h2'), DialogDescription: box('p') }
})

const lone = { ...mesh, peers: [] as never }

test('not in a mesh: no mesh copy at all', () => {
  expect(shareCopy(lone, 'X', undefined, 'editor', false)).toBeNull()
  expect(shareCopy(undefined, null, undefined, 'editor', false)).toBeNull()
})

test('homed, via home: served by the home, or by the mesh when it is offline; viewer reads can view', () => {
  const c = shareCopy(mesh, 'X', 'home', 'editor', false)
  expect(c).toEqual({ text: 'Anyone with this link can edit the note. It is served by xeus; if xeus is offline, by your mesh.', small: 'Older links keep working.' })
  expect(shareCopy(mesh, 'X', undefined, 'viewer', false)?.text).toContain('can view the note')
})

test('homed, via mesh: the mesh passes it to the home', () => {
  expect(shareCopy(mesh, 'X', 'mesh', 'editor', false)).toEqual({
    text: 'Anyone with this link can edit the note. It is served by your mesh, which passes it to xeus.',
    small: 'Link sharing is off on xeus, so this is a mesh link. Older links keep working.',
  })
})

test('mesh_link_unavailable: no hub is reachable', () => {
  expect(shareCopy(mesh, 'X', undefined, 'editor', true)).toEqual({
    text: 'Link sharing is off on xeus, and no hub is reachable to make a mesh link.',
    small: 'Turn on Settings › Link on xeus, or retry when the hub is back.',
  })
})

test('unhomed in a mesh: served by the mesh', () => {
  expect(shareCopy(mesh, null, 'home', 'editor', false)).toEqual({ text: 'Anyone with this link can edit the document. It is served by your mesh.', small: 'Older links keep working.' })
})

const render = (home: string | null) =>
  renderToStaticMarkup(
    createElement(QueryClientProvider, {
      client: new QueryClient(),
      children: createElement(ShareDialog, {
        provider: { id: 'd1', meta: { title: 'Plan', home }, onEvent: () => () => {} } as unknown as EngineProvider,
        open: true,
        onOpenChange: () => {},
      }),
    }),
  )

test('markup: a mesh titles the dialog with the note and shows the case; a lone engine keeps today\'s copy', () => {
  const html = render('X')
  expect(html).toContain('Share &quot;Plan&quot;')
  expect(html).toContain('It is served by xeus; if xeus is offline, by your mesh.')
  expect(html).toContain('Older links keep working.')
  current = lone
  const single = render('X')
  expect(single).toContain('<h2>Share</h2>')
  expect(single).toContain('Anyone with a link connects to this Mac through the relay and edits live; a viewer link only shows the document.')
  expect(single).not.toContain('Older links')
  current = mesh
})
