import { expect, test } from 'vitest'
import { swallowEngineQueries } from './terminal-replies'

type Params = (number | number[])[]
type Id = { prefix?: string; intermediates?: string; final: string }

/** A hand `term.parser`: records handlers, dispatches like xterm (exact identifier match, else unhandled). */
function parser() {
  const csi: [Id, (p: Params) => boolean | Promise<boolean>][] = []
  const osc = new Map<number, (d: string) => boolean | Promise<boolean>>()
  const off = { dispose() {} }
  const stub = {
    registerCsiHandler(id: Id, cb: (p: Params) => boolean | Promise<boolean>) {
      csi.push([id, cb])
      return off
    },
    registerOscHandler(ident: number, cb: (d: string) => boolean | Promise<boolean>) {
      osc.set(ident, cb)
      return off
    },
  }
  swallowEngineQueries(stub)
  const same = (a: Id, b: Id) => (a.prefix ?? '') === (b.prefix ?? '') && (a.intermediates ?? '') === (b.intermediates ?? '') && a.final === b.final
  return {
    /** True when the viewer swallows the CSI (xterm would not answer it). */
    csi: (id: Id, params: Params) => csi.some(([i, cb]) => same(i, id) && cb(params) === true),
    osc: (ident: number, data: string) => osc.get(ident)?.(data) === true,
  }
}

test('the queries the engine answers are swallowed', () => {
  const p = parser()
  expect(p.csi({ final: 'c' }, [0])).toBe(true) // CSI c, CSI 0 c
  expect(p.csi({ final: 'c' }, [])).toBe(true)
  expect(p.csi({ prefix: '>', final: 'c' }, [0])).toBe(true) // CSI > c, CSI > 0 c
  expect(p.csi({ final: 'n' }, [5])).toBe(true)
  expect(p.csi({ final: 'n' }, [6])).toBe(true)
  expect(p.osc(10, '?')).toBe(true)
  expect(p.osc(11, '?')).toBe(true)
})

test('everything else reaches xterm', () => {
  const p = parser()
  expect(p.csi({ final: 'c' }, [1])).toBe(false)
  expect(p.csi({ prefix: '>', final: 'c' }, [1])).toBe(false)
  expect(p.csi({ prefix: '=', final: 'c' }, [0])).toBe(false) // DA3
  expect(p.csi({ prefix: '?', final: 'n' }, [6])).toBe(false) // DECXCPR
  expect(p.csi({ final: 'n' }, [5, 6])).toBe(false)
  expect(p.csi({ final: 'n' }, [[6]])).toBe(false)
  expect(p.csi({ prefix: '?', intermediates: '$', final: 'p' }, [2026])).toBe(false) // DECRQM
  expect(p.osc(10, 'rgb:ffff/ffff/ffff')).toBe(false) // colour SET
  expect(p.osc(11, '#000000')).toBe(false)
  expect(p.osc(10, '?;?')).toBe(false)
  expect(p.osc(12, '?')).toBe(false)
})
