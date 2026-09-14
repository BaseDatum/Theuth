// 25 §6.3 / §9 step 8: start N sessions streaming at once against a daemon and
// print the event rate, so a DevTools trace of the app has a known load.
//   bun crates/desktop/perf_fixture.ts <data_dir> <port> [n=50]
// The check: with one session open in the app, no frame gap > 50 ms and no
// long task for the duration (measured 2026-09-15: 59.9 fps, worst 25 ms, 0
// long tasks at ~700 deltas/s, 50 sessions, heap 81 MB).
const [dataDir, port, count] = process.argv.slice(2)
const tok = (await Bun.file(`${dataDir}/token`).text()).trim()
const n = Number(count ?? 50)
const ws = new WebSocket(`ws://127.0.0.1:${port}/?token=${tok}`)
let id = 0
const pending = new Map<number, (v: unknown) => void>()
const call = (method: string, params: unknown[]) =>
  new Promise<unknown>((res) => {
    pending.set(++id, res)
    ws.send(JSON.stringify({ jsonrpc: '2.0', id, method, params }))
  })
let events = 0
let deltas = 0
const finished = new Set<string>()
ws.onmessage = (m) => {
  const j = JSON.parse(String(m.data))
  if (j.id) pending.get(j.id)?.(j.result ?? j.error)
  else if (j.method === 'subscribe_events') {
    events++
    const e = j.params.result
    if (e.type === 'TextDelta') deltas++
    if (e.type === 'TurnFinished') finished.add(e.session)
  }
}
await new Promise((r) => (ws.onopen = r))
await call('subscribe_events', [null, null])
const t0 = Date.now()
await Promise.all(
  Array.from({ length: n }, (_, i) =>
    call('send_input', [null, `Write a 400-word essay about river number ${i}. No tools.`, []]),
  ),
)
console.log(`${n} sessions started in ${Date.now() - t0} ms`)
const timer = setInterval(() => {
  console.log(`t+${((Date.now() - t0) / 1000).toFixed(0)}s events=${events} deltas=${deltas} finished=${finished.size}/${n}`)
  if (finished.size >= n) {
    clearInterval(timer)
    ws.close()
  }
}, 2000)
