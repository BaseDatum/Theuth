import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import type { ElkNode } from 'elkjs/lib/elk-api.js'
import { useElkLayout } from '../graph-layout'
import { elkLayout, elkPeek } from './elk'

// The worker-backed ELK stands in with an echo: the layout is whatever `map` makes of the built graph.
vi.mock('elkjs/lib/elk-api.js', () => ({ default: class { layout = (root: ElkNode) => Promise.resolve(root) } }))
vi.stubGlobal('document', { fonts: { ready: Promise.resolve() } })

const root: ElkNode = { id: 'root', children: [{ id: 'a', width: 10, height: 10 }] }
const map = (r: ElkNode) => ({ n: r.children?.length ?? 0 })

function Probe({ hash }: { hash: string }) {
  const shown = useElkLayout(hash, () => root, map)
  return createElement('i', null, shown && 'layout' in shown ? `hit ${shown.layout.n}` : 'miss')
}

test('elkPeek: the LRU without a promise; a laid-out hash is there synchronously', async () => {
  expect(elkPeek('h1')).toBeUndefined()
  expect(await elkLayout('h1', () => root, map)).toEqual({ n: 1 })
  expect(elkPeek<{ n: number }>('h1')).toEqual({ n: 1 })
})

test('useElkLayout (R2.3): a cached hash is shown on the first render, a miss starts null', async () => {
  expect(renderToStaticMarkup(createElement(Probe, { hash: 'h2' }))).toBe('<i>miss</i>')
  await elkLayout('h2', () => root, map)
  expect(renderToStaticMarkup(createElement(Probe, { hash: 'h2' }))).toBe('<i>hit 1</i>')
})
