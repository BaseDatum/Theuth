// Owner (218): "Why not just the graph card?" A whiteboard diagram card is the frame; inside it the chat's renderers draw
// bare: no json-render Card around the graph, no diagram frame border or `graph` label, and the diagram fills the box.
import '../../settings/test-dom'
import { createElement as h } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import type { Spec } from '@json-render/react'
import type { CustomRendererProps } from 'streamdown'
import { BareSurface, unwrapRoot } from './bare'
import { JsonRenderFence } from './JsonRenderFence'

const graph = { type: 'Graph', props: { nodes: [{ id: 'a', label: 'Cart' }, { id: 'b', label: 'Pay' }], edges: [{ from: 'a', to: 'b' }] } }
const WRAPPED: Spec = {
  root: 'card',
  elements: {
    card: { type: 'Card', props: { title: 'Checkout flow' }, children: ['stack'] },
    stack: { type: 'Stack', props: { direction: 'vertical' }, children: ['g'] },
    g: graph,
  },
} as unknown as Spec

const fence = (spec: Spec) => ({ code: JSON.stringify(spec), isIncomplete: false, language: 'json-render' }) as unknown as CustomRendererProps
const render = (bare: boolean) => {
  const el = document.createElement('div')
  el.innerHTML = renderToStaticMarkup(h(BareSurface.Provider, { value: bare }, h(JsonRenderFence, fence(WRAPPED))))
  return el
}

test('a root Card or Stack that only wraps gives way to what it holds; a Card of several becomes a plain Stack', () => {
  expect(unwrapRoot(WRAPPED).root).toBe('g')
  const two = { root: 'c', elements: { c: { type: 'Card', props: { title: 'T' }, children: ['g', 'h'] }, g: graph, h: graph } } as unknown as Spec
  expect(unwrapRoot(two).elements.c).toMatchObject({ type: 'Stack', children: ['g', 'h'] })
  expect(unwrapRoot(two).elements.c!.props).not.toHaveProperty('title')
  const lone = { root: 'g', elements: { g: graph } } as unknown as Spec
  expect(unwrapRoot(lone)).toBe(lone)
})

test('in the chat the graph keeps its Card and its diagram frame', () => {
  const el = render(false)
  expect(el.textContent).toContain('Checkout flow')
  expect(el.querySelector('[data-streamdown="graph-block"]')!.className).toContain('border')
})

test('on a whiteboard card the graph is the whole surface: no Card, no title, no frame border or label, filling the card', () => {
  const el = render(true)
  expect(el.textContent).not.toContain('Checkout flow')
  expect(el.textContent).not.toMatch(/^graph/)
  expect(el.querySelector('.rounded-xl')).toBeNull()
  const surface = el.querySelector<HTMLElement>('[data-streamdown="json-render"]')!
  expect(surface.className).toContain('h-full')
  const frame = surface.querySelector<HTMLElement>('[data-streamdown="graph-block"]')!
  // The diagram frame is the surface's only content, and it fills: no border, no inline margin, full height.
  expect(frame.dataset.bare).toBe('true')
  expect(frame.className).toContain('h-full')
  expect(frame.className).not.toMatch(/\bborder\b|\bmy-4\b|\bp-2\b/)
  expect(frame.parentElement!.className).toContain('h-full')
})
