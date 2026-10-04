import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { createElement, type ReactNode } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import type * as Rpc from '@/lib/rpc'
import { parseScreenLine } from '@/lib/screen-context'
import type * as Conn from '@/store/connection'
import { imagesFor, MessageUser } from './MessageUser'
import { coords, ScreenContextCard, verb } from './ScreenContextCard'

vi.mock('@tanstack/react-router', () => ({
  useNavigate: () => vi.fn(),
  Link: ({ children }: { children: ReactNode }) => createElement('a', null, children),
}))
vi.mock('@/lib/rpc', async (original) => ({
  ...(await original<typeof Rpc>()),
  // Not the local daemon here, so a thumbnail waits on `get_attachment`; never answering keeps the placeholder on screen.
  useApi: () => ({ get_attachment: () => Promise.withResolvers<string>().promise }),
}))
// `usePointerCoarse` is a `useSyncExternalStore` on `matchMedia` with no server snapshot.
vi.mock('@/lib/mobile', () => ({ usePointerCoarse: () => false }))
// `renderToStaticMarkup` reads zustand's initial state, so the engine's `data_dir` is a plain pick here.
vi.mock('@/store/connection', async (original) => ({
  ...(await original<typeof Conn>()),
  useConnection: (pick: (s: unknown) => unknown) => pick({ info: { data_dir: '/dd' }, api: null }),
}))

const WIN = 'window "majiayu000/litellm-rs: Self-hosted Rust LLM gateway with OpenAI-compatible APIs,…"'
const SKY = `[voice note, 3 s] Can you clone this repo\n[screen 1/1 "theuth" · pointer: circled (153,91)–(831,355): ${WIN}; underlined (630,359)→(714,360): ${WIN}; rested on (711,358) for 1849 ms: ${WIN}; ended on ${WIN} · accessibility: on][image: eec889cc446240df.png][image: 31d922047487392b.png][image: 53cedad67f01b3e3.png][image: f330ce422f86eeb0.png]`

const render = (child: ReactNode) =>
  renderToStaticMarkup(createElement(QueryClientProvider, { client: new QueryClient(), children: createElement(TooltipProvider, null, child) }))

test("Sky's message: words in the bubble, one card, four plain-word gesture rows, four thumbnails, no raw line or token", () => {
  const html = render(createElement(MessageUser, { text: SKY, session: 's1', turn: 3 }))
  expect(html).toContain('Can you clone this repo')
  expect(html).toContain('3 s')
  expect(html).toContain('Pointed at the screen')
  expect(html).toContain('>theuth<')
  expect(html).toContain('Accessibility on')
  for (const v of ['Circled', 'Underlined', 'Rested 1.8 s on', 'Ended on']) expect(html).toContain(`>${v}<`)
  // Coordinates are tooltips, never text.
  expect(html).toContain('title="(153,91)–(831,355)"')
  expect(html).toContain('title="(711,358) · 1849 ms"')
  expect(html.replace(/title="[^"]*"/g, '')).not.toMatch(/\(\d+,\d+\)/)
  expect(html).not.toContain('[screen')
  expect(html).not.toContain('[image:')
  expect(html).not.toContain('accessibility: on')
  expect(html.match(/>window</g)).toHaveLength(4)
  for (const name of ['eec889cc446240df.png', '31d922047487392b.png', '53cedad67f01b3e3.png', 'f330ce422f86eeb0.png']) expect(html).toContain(`aria-label="Open image: ${name}"`)
  // No `1/1` badge for a single display.
  expect(html).not.toContain('>1/1<')
})

test('§6.6 card: title tooltip, url as an external link, link chips open their url, a 2/2 badge; a bare `ended on (x,y)` is the verb alone', () => {
  const s = parseScreenLine(
    '[screen 2/2 "Google Chrome" · "basedatum — GitHub" · https://github.com/basedatum?tab=repositories · pointer: circled (612,418)–(884,452): link "theuth" → https://github.com/basedatum/theuth, text "Rust · Updated 2 hours ago"; ended on (1,2) · accessibility: off]',
  )!
  const html = renderToStaticMarkup(createElement(ScreenContextCard, { screen: s, files: ['/dd/attachments/a.png'] }))
  expect(html).toContain('title="basedatum — GitHub"')
  expect(html).toContain('href="https://github.com/basedatum?tab=repositories" target="_blank" rel="noopener noreferrer"')
  expect(html).toContain('github.com/basedatum?tab=repositories<')
  expect(html).toContain('href="https://github.com/basedatum/theuth" target="_blank"')
  expect(html).toContain('Rust · Updated 2 hours ago')
  expect(html).toContain('>2/2<')
  expect(html).toContain('Accessibility off')
  expect(html).toContain('>Ended<')
  expect(html).toContain('aria-label="Open image: a.png"')
})

test('verb and coords per gesture kind', () => {
  expect(verb({ kind: 'rested', at: [1, 1], ms: 640, elements: [] })).toBe('Rested 0.6 s')
  expect(verb({ kind: 'rested', at: [1, 1], ms: 1849, elements: [{ role: 'text' }] })).toBe('Rested 1.8 s on')
  expect(coords({ kind: 'underlined', from: [1, 2], to: [3, 4], elements: [] })).toBe('(1,2)→(3,4)')
  expect(coords({ kind: 'ended', elements: [{ role: 'link' }] })).toBeUndefined()
})

test('imagesFor: every image on a lone card; overview per card and the crops on the last', () => {
  const xs = ['o1', 'o2', 'c1', 'c2']
  expect(imagesFor(xs, 0, 1)).toBe(xs)
  expect(imagesFor(xs, 0, 2)).toEqual(['o1'])
  expect(imagesFor(xs, 1, 2)).toEqual(['o2', 'c1', 'c2'])
  expect(imagesFor(['o1'], 1, 2)).toEqual([])
  expect(imagesFor(undefined, 0, 2)).toBeUndefined()
})

test('a history image message without a screen line: thumbnails from the tokens, the bubble without them', () => {
  const html = render(createElement(MessageUser, { text: 'look [image: ab.png]', session: 's1' }))
  expect(html).toContain('aria-label="Open image: ab.png"')
  expect(html).toMatch(/>look<\/div>/)
  expect(html).not.toContain('Pointed at the screen')
})
