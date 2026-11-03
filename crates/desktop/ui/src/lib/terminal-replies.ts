import type { IParser } from '@xterm/xterm'

// One responder (167 D2): the engine answers these terminal queries itself (pty.rs `QUERIES` + CPR, 22 §4.4), so a
// viewer's xterm must not answer them again through `onData`. Each handler returns true (swallowed) only for the
// exact query form; anything else falls through to xterm's own handler (DECRQM, `CSI ?6n`, colour SETs, …).

type Params = (number | number[])[]

/** No parameter, or a single 0 (xterm parses `CSI c` as `[0]`). */
const zero = (p: Params) => p.length === 0 || (p.length === 1 && p[0] === 0)

/** DA1, DA2, DSR 5/6 and OSC 10/11 `?` queries the engine answers: swallowed by this terminal's parser. */
export function swallowEngineQueries(parser: Pick<IParser, 'registerCsiHandler' | 'registerOscHandler'>): void {
  parser.registerCsiHandler({ final: 'c' }, zero)
  parser.registerCsiHandler({ prefix: '>', final: 'c' }, zero)
  parser.registerCsiHandler({ final: 'n' }, (p) => p.length === 1 && (p[0] === 5 || p[0] === 6))
  for (const osc of [10, 11]) parser.registerOscHandler(osc, (data) => data === '?')
}
