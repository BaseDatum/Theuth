/**
 * 73 §5.2 — the four lens methods behind one interface. `rpc` calls the engine by flat name through `RpcClient.call`
 * (not `Api`/`METHODS`: those mirror `bindings/methods.json`, which the engine worker extends when E2 lands);
 * `mock` serves the §5 fixtures so the panel builds and smokes before the engine does.
 * Dev flag: `VITE_CODEMAP_MOCK=1` (or `localStorage.codemap-mock = "1"`) picks the mock.
 */
import changeFixture from '@/components/codemap/__fixtures__/change.json'
import fitFixture from '@/components/codemap/__fixtures__/fit.json'
import hereFixture from '@/components/codemap/__fixtures__/here.json'
import type { RpcClient } from '@/rpc/client'
import type { ChangeLens, CodemapApi, FitLens, FitNode, HereLens, LensMeta } from './types'

export const codemapMock = (): boolean =>
  import.meta.env.DEV && (import.meta.env.VITE_CODEMAP_MOCK === '1' || globalThis.localStorage?.getItem('codemap-mock') === '1')

export const rpcCodemap = (client: RpcClient): CodemapApi => ({
  codemap_here: (root, focus, edited, limit) => client.call('codemap_here', [root, focus, edited, limit]),
  codemap_fit: (root, symbol, limit) => client.call('codemap_fit', [root, symbol, limit]),
  codemap_change: (root, git_range, files, include_tests) => client.call('codemap_change', [root, git_range, files, include_tests]),
  codemap_touch: (root, paths) => client.call('codemap_touch', [root, paths]),
})

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
