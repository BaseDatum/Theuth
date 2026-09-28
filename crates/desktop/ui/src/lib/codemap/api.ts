/**
 * 73 §5.2 — the four lens methods behind one interface. The engine's `Api` implements `CodemapApi`
 * structurally (`codemap_here/fit/change/touch`, flat names, positional params); `mock` serves the §5
 * fixtures so the panel can be developed without an engine.
 * Dev flag: `VITE_CODEMAP_MOCK=1` (or `localStorage.codemap-mock = "1"`) picks the mock (pair it with
 * `dev/codemap-fixture.ts`, which replays the fixture events).
 */
import changeFixture from '@/components/codemap/__fixtures__/change.json'
import fitFixture from '@/components/codemap/__fixtures__/fit.json'
import hereFixture from '@/components/codemap/__fixtures__/here.json'
import type { ChangeLens, CodemapApi, FitLens, FitNode, HereLens, LensMeta } from './types'

export const codemapMock = (): boolean =>
  import.meta.env.DEV && (import.meta.env.VITE_CODEMAP_MOCK === '1' || globalThis.localStorage?.getItem('codemap-mock') === '1')

/** No engine connection: every lens rejects; the panel shows the error line and retries on reconnect. */
export const disconnectedCodemap: CodemapApi = {
  codemap_here: () => Promise.reject(new Error('not connected')),
  codemap_fit: () => Promise.reject(new Error('not connected')),
  codemap_change: () => Promise.reject(new Error('not connected')),
  codemap_touch: () => Promise.reject(new Error('not connected')),
}

const clone = <T,>(v: T): T => structuredClone(v)
const FIT = fitFixture as FitLens
let seq = 100

/** The fixture re-centred on `symbol` when it names a neighbour (name or moniker): the old focus becomes the one neighbour on the other side, so a smoke can watch surviving nodes stay put. */
export function mockFit(symbol: string): FitLens {
  const fit = clone(FIT)
  const hit = (ns: FitNode[]) => ns.find((n) => n.moniker === symbol || n.name === symbol || n.name.endsWith(`::${symbol}`))
  const caller = hit(fit.callers)
  const callee = hit(fit.callees)
  if (!caller && !callee) return { ...fit, meta: { ...fit.meta, seq: ++seq } }
  const focus = (caller ?? callee)!
  const keep = (ns: FitNode[]) => ns.filter((n) => n.moniker !== focus.moniker)
  const edges = fit.edges.filter((e) => e.from === focus.moniker || e.to === focus.moniker)
  return {
    ...fit,
    meta: { ...fit.meta, seq: ++seq },
    focus,
    callers: caller ? [] : keep(fit.callers),
    callees: caller ? [fit.focus] : [],
    edges: caller ? edges : [...edges, ...fit.edges.filter((e) => e.to === fit.focus.moniker)],
    ambiguous: 0,
    more_callers: caller ? 0 : fit.more_callers,
    more_callees: caller ? 0 : 0,
  }
}

export const mockCodemap = (): CodemapApi => ({
  async codemap_here(root, focus, edited) {
    const here = clone(hereFixture as HereLens)
    here.meta.root = root
    here.meta.seq = ++seq
    here.personalised = !!edited?.length
    if (focus?.length) here.rows.sort((a, b) => Number(focus.some((f) => b.path.startsWith(f))) - Number(focus.some((f) => a.path.startsWith(f))))
    return here
  },
  async codemap_fit(root, symbol) {
    const fit = mockFit(symbol)
    fit.meta.root = root
    return fit
  },
  async codemap_change(root, git_range, files, include_tests) {
    const change = clone(changeFixture as unknown as ChangeLens)
    change.meta.root = root
    change.meta.seq = ++seq
    if (files?.length) change.mode = 'files'
    else if (git_range) {
      change.mode = 'range'
      const [old = git_range, nu = 'HEAD'] = git_range.split('..')
      change.range = [old, nu]
    }
    if (include_tests === false) change.tests = []
    return change
  },
  async codemap_touch(root): Promise<LensMeta> {
    return { ...(hereFixture as HereLens).meta, root, seq: ++seq, ms: 3 }
  },
})
