import { readFileSync } from 'node:fs'
import { expect, test, vi } from 'vitest'
import type { Located } from '@/rpc/bindings/Located'
import { excerptLines } from '@/components/chat/surface/parse'
import { type DocBlock, excerptsOf } from './blocks'
import { chipText, collectRequests, createLiveStore, excerptKey, langOf, sha256, staleOf } from './live'

const FIXTURE = readFileSync(new URL('./fixtures/surface-fence-parsing.md', import.meta.url), 'utf8')
const fences = [...FIXTURE.matchAll(/```json-render\n([\s\S]*?)\n```/g)].map((m) => m[1]!)
const codeBlock = (id: string, text: string): DocBlock =>
  ({ id, type: 'codeBlock', props: { language: 'json-render' }, content: [{ type: 'text', text, styles: {} }], children: [] }) as unknown as DocBlock
const para = (id: string, hrefs: string[]): DocBlock =>
  ({
    id,
    type: 'paragraph',
    props: {},
    content: hrefs.map((href) => ({ type: 'link', href, content: [{ type: 'text', text: 'x', styles: {} }] })),
    children: [],
  }) as unknown as DocBlock
const row = (status: Located['status'], start: number, end: number, text: string | null = 'x'): Located => ({ status, start, end, text })

test('the batch carries every excerpt (hashed, with symbol), every code: link and every unit ref once, in document order', async () => {
  const blocks = [...fences.map((f, i) => codeBlock(`b${i}`, f)), para('p', ['code:crates/surface/src/fences.rs#L90-L94', 'code:crates/surface/src/fences.rs#L90-L94', 'https://x.io', 'code:crates/surface/src/lib.rs'])]
  const reqs = await collectRequests(blocks)
  const excerpts = reqs.filter((r) => r.key.startsWith('excerpt:'))
  expect(excerpts.map((r) => [r.key, r.anchor.start, r.anchor.end])).toEqual([
    ['excerpt:b2', 74, 87],
    ['excerpt:b3', 89, 95],
    ['excerpt:b4', 38, 59],
  ])
  const first = excerptsOf(blocks)[0]!.props.text
  expect(excerpts[0]!.anchor.text_sha).toBe(await sha256(excerptLines(first).join('\n')))
  expect(excerpts.every((r) => r.anchor.text_sha?.length === 64)).toBe(true)
  const refs = reqs.filter((r) => !r.key.startsWith('excerpt:'))
  // Graph 6 + Sequence 7 refs in the fixture, deduplicated with the link that repeats one of them.
  expect(refs.length).toBeGreaterThanOrEqual(8)
  expect(refs.filter((r) => r.key === 'crates/surface/src/fences.rs#L90-L94')).toHaveLength(1)
  expect(refs.at(-1)).toEqual({ key: 'crates/surface/src/lib.rs', anchor: { path: 'crates/surface/src/lib.rs', start: null, end: null, symbol: null, text_sha: null } })
  expect(refs.every((r) => r.anchor.text_sha === null)).toBe(true)
})

test('sha256 matches codeindex::locate::text_sha (lines joined by \\n, lowercase hex)', async () => {
  // `printf '…' | shasum -a 256`; the first is the 7a wire smoke's excerpt.
  expect(await sha256('pub fn helper() -> u32 {\n    1\n}')).toBe('c652902763db3643b251065363edf41afee7563e5d00e5f1293a40dc93cd9514')
  expect(await sha256('a\nb')).toBe('7e18f737311b2dc3b2f269dd78396b0351f14fb66efa879f768cb23181883c78')
})

test('ensure locates only what is not cached, verify refetches everything; stale counts every non-same row', async () => {
  const locate = vi.fn(async (anchors: { path: string }[]) => anchors.map((_, i) => row(i === 0 ? 'changed' : 'same', 1, 2)))
  const live = createLiveStore(locate)
  const reqs = [
    { key: excerptKey('e1'), anchor: { path: 'a.rs', start: 1, end: 2, symbol: null, text_sha: 'x' } },
    { key: 'a.rs#L5', anchor: { path: 'a.rs', start: 5, end: 5, symbol: null, text_sha: null } },
  ]
  live.getState().ensure(reqs)
  expect(live.getState().pending).toBe(1)
  await vi.waitFor(() => expect(live.getState().pending).toBe(0))
  expect(locate).toHaveBeenCalledTimes(1)
  expect(locate.mock.calls[0]![0]).toHaveLength(2)
  expect(live.getState().results.get(excerptKey('e1'))?.status).toBe('changed')
  expect(staleOf(live.getState().results, reqs)).toBe(1)

  // A Yjs insert adds one anchor: only it goes over the wire.
  const more = [...reqs, { key: 'b.rs#L1-L3', anchor: { path: 'b.rs', start: 1, end: 3, symbol: null, text_sha: null } }]
  live.getState().ensure(more)
  await vi.waitFor(() => expect(live.getState().pending).toBe(0))
  expect(locate).toHaveBeenCalledTimes(2)
  expect(locate.mock.calls[1]![0]).toEqual([more[2]!.anchor])
  live.getState().ensure(more) // all cached: no call
  expect(locate).toHaveBeenCalledTimes(2)

  // Verify: cache dropped, the whole batch again.
  live.getState().verify(more)
  expect(live.getState().results.size).toBe(0)
  await vi.waitFor(() => expect(live.getState().pending).toBe(0))
  expect(locate).toHaveBeenCalledTimes(3)
  expect(locate.mock.calls[2]![0]).toHaveLength(3)
  expect(live.getState().results.size).toBe(3)
  expect(live.getState().verifiedAt).not.toBeNull()
})

test('a failed batch (root not a git top-level, path outside root) surfaces as error and clears on the next success', async () => {
  const locate = vi.fn<(a: unknown[]) => Promise<Located[]>>().mockRejectedValueOnce(new Error('root /tmp is not a git top-level')).mockResolvedValueOnce([row('same', 1, 1)])
  const live = createLiveStore(locate)
  const reqs = [{ key: 'a.rs#L1', anchor: { path: 'a.rs', start: 1, end: 1, symbol: null, text_sha: null } }]
  live.getState().ensure(reqs)
  await vi.waitFor(() => expect(live.getState().pending).toBe(0))
  expect(live.getState().error).toBe('root /tmp is not a git top-level')
  expect(live.getState().results.size).toBe(0)
  live.getState().verify(reqs)
  await vi.waitFor(() => expect(live.getState().pending).toBe(0))
  expect(live.getState().error).toBeNull()
  expect(live.getState().results.get('a.rs#L1')?.status).toBe('same')
})

test('chip text: nothing for same, changed, moved with the new range, missing; langOf guesses from the extension', () => {
  expect(chipText(undefined)).toBeNull()
  expect(chipText(row('same', 1, 1))).toBeNull()
  expect(chipText(row('changed', 1, 3))).toBe('changed')
  expect(chipText(row('moved', 88, 101))).toBe('moved → L88–L101')
  expect(chipText(row('moved', 7, 7))).toBe('moved → L7')
  expect(chipText(row('missing', 1, 1, null))).toBe('missing')
  expect(langOf('crates/surface/src/lib.rs')).toBe('rs')
  expect(langOf('Makefile')).toBeUndefined()
  expect(langOf('a.b/Makefile')).toBeUndefined()
})
