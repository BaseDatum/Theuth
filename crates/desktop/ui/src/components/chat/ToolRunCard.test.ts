import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { disclosureKey, useToolRuns } from '@/lib/tool-runs'
import type { Entry } from '@/store/sessions'
import { foldRuns, isDeliverable, summarize, summarizeCells, summarizeGroup, ToolRunCard } from './ToolRunCard'

vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: unknown }) => children }))

type Tool = Extract<Entry, { kind: 'tool' }>
const tool = (n: number, extra: Partial<Tool> = {}): Tool => ({
  id: `e${n}`,
  kind: 'tool',
  callId: `c${n}`,
  name: `t${n}`,
  args: { n },
  pending: false,
  child: false,
  result: { output: `out ${n}\nmore`, isError: false, files: [] },
  ...extra,
})
const text = (id: string): Entry => ({ id, kind: 'assistant', text: 'so', open: false })

test('foldRuns: assistant text splits runs, reasoning rides along, a lone call stays a plain entry', () => {
  const think: Entry = { id: 'r', kind: 'reasoning', text: 'hm', done: true }
  const [a, b, c, d, e, f] = [tool(1), tool(2), text('x'), tool(3), think, tool(4)]
  expect(foldRuns([a, b, c, d, e, f])).toEqual([[a, b], c, [d, e, f]])
  expect(foldRuns([c, a, c])).toEqual([c, a, c])
  // A trailing reasoning row is not part of the run.
  expect(foldRuns([a, b, e])).toEqual([[a, b], e])
})

test('183 §12.4: foldRuns splits a run at a call that carries a restart note, so its card stays in view', () => {
  const [a, b, c, d] = [tool(1), tool(2), tool(3), tool(4)]
  const restarted = tool(9, { restart: { clock: '09:00:00', after: 9, cause: 'restart', replayed: 1, interrupted: 0, text: '' } })
  expect(foldRuns([a, b, restarted, c, d])).toEqual([[a, b], restarted, [c, d]])
  expect(foldRuns([restarted, a, restarted])).toEqual([restarted, a, restarted])
})

test('foldRuns: a pinned thinking block splits a run like a deliverable; unpinned still rides along; a lone tool beside it stays plain', () => {
  const think: Entry = { id: 'r', kind: 'reasoning', text: 'hm', done: false }
  const other: Entry = { id: 'r2', kind: 'reasoning', text: 'ok', done: true }
  const [a, b, c, d] = [tool(1), tool(2), tool(3), tool(4)]
  const pinned = new Set(['r'])
  expect(foldRuns([a, b, think, c, d], pinned)).toEqual([[a, b], think, [c, d]])
  expect(foldRuns([a, b, other, c, d], pinned)).toEqual([[a, b, other, c, d]])
  // A lone tool on either side of the pinned block is a plain entry, not a one-tool run.
  expect(foldRuns([a, think, c, d], pinned)).toEqual([a, think, [c, d]])
  expect(foldRuns([a, b, think, c], pinned)).toEqual([[a, b], think, c])
  // The streaming tail: pinned or not, a trailing block is outside the run.
  expect(foldRuns([a, b, think], pinned)).toEqual([[a, b], think])
  // Deliverables still split as before alongside a pin; no pins, the old shape.
  const s = tool(9, { name: 'show' })
  expect(foldRuns([a, s, b, think, c, d], pinned)).toEqual([a, s, b, think, [c, d]])
  expect(foldRuns([a, b, think, c, d])).toEqual([[a, b, think, c, d]])
})

test('foldRuns: a deliverable never joins a run — it splits one; a board create still does; no deliverable, no change', () => {
  const [a, b, c, d] = [tool(1), tool(2), tool(3), tool(4)]
  for (const name of ['show', 'app', 'agent_draft', 'task_draft', 'routine_draft', 'send_message']) {
    const s = tool(9, { name })
    expect(isDeliverable(s)).toBe(true)
    expect(foldRuns([a, b, s, c, d])).toEqual([[a, b], s, [c, d]])
    expect(foldRuns([a, b, s, c])).toEqual([[a, b], s, c])
    expect(foldRuns([s, a, s])).toEqual([s, a, s])
  }
  const board = tool(8, { name: 'doc_create', args: { kind: 'board', title: 'T' } })
  expect(isDeliverable(board)).toBe(true)
  expect(foldRuns([a, board, b, c])).toEqual([a, board, [b, c]])
  expect(isDeliverable(tool(7, { name: 'doc_create', args: { kind: 'note' } }))).toBe(false)
  expect(foldRuns([a, b, c, d])).toEqual([[a, b, c, d]])
})

test('foldRuns: human-input cards that stay visible split a run and a lone one between text rows stays a plain entry', () => {
  const [a, b, c, d] = [tool(1), tool(2), tool(3), tool(4)]
  const ask = tool(9, { name: 'ask_user', args: { questions: [{ id: 'q', text: 'Which?' }] }, result: { output: 'answered', isError: false, files: [] } })
  expect(isDeliverable(ask)).toBe(true)
  expect(foldRuns([a, b, ask, c, d])).toEqual([[a, b], ask, [c, d]])
  expect(foldRuns([text('x'), ask, text('y')])).toEqual([text('x'), ask, text('y')])
  const handoff = tool(8, { name: 'desktop_handoff', args: { reason: 'Log in to GitHub' } })
  expect(isDeliverable(handoff)).toBe(true)
  expect(foldRuns([a, b, handoff, c, d])).toEqual([[a, b], handoff, [c, d]])
  expect(isDeliverable(tool(8, { name: 'secret_request' }))).toBe(false)
})

type Peer = Extract<Entry, { kind: 'peer' }>
const peer = (id: string, from: string, body = id, extra: Partial<Peer> = {}): Peer => ({ id, kind: 'peer', from, text: body, ...extra })

test('foldRuns: adjacent peer cards from one sender fold; another sender, any other row or a removed row ends the fold', () => {
  const [a1, a2, a3] = [peer('a1', 'session:a'), peer('a2', 'session:a'), peer('a3', 'session:a')]
  const b = peer('b', 'session:b')
  expect(foldRuns([a1, a2, a3, b])).toEqual([[a1, a2, a3], b])
  expect(foldRuns([a1, text('x'), a2])).toEqual([a1, text('x'), a2])
  expect(foldRuns([a1, b, a2])).toEqual([a1, b, a2])
  const removed = peer('r', 'session:a', 'r', { context: { target: { kind: 'message', seq: 3 }, active: false, removed: true, remove_with: [], attachments: [] } } as Partial<Peer>)
  expect(foldRuns([a1, removed, a2])).toEqual([a1, removed, a2])
  // The authenticated endpoint decides, not the label.
  const o1 = peer('o1', 'label', 'x', { origin: { id: 'm1', from: 'session:a', reply_to: null, scope: null, contact_mode: null } })
  const o2 = peer('o2', 'other label', 'y', { origin: { id: 'm2', from: 'session:a', reply_to: null, scope: null, contact_mode: null } })
  expect(foldRuns([o1, o2])).toEqual([[o1, o2]])
  // A send between two of its answers keeps them apart and stays its own card.
  const send = tool(5, { name: 'send_message', args: { to: 'session:a', message: 'hi' } })
  expect(foldRuns([tool(1), send, tool(2), a1, send, a2])).toEqual([tool(1), send, tool(2), a1, send, a2])
})

test('foldRuns: a job result folds like a tool card — beside tools, with another job, never alone or into a peer group', () => {
  const [a, b, c] = [tool(1), tool(2), tool(3)]
  const [j1, j2] = [peer('j1', 'job j4', 'out'), peer('j2', 'j4 · make · exit 0 · 3s', 'out')]
  const think: Entry = { id: 'r', kind: 'reasoning', text: 'hm', done: true }
  expect(foldRuns([a, j1, b])).toEqual([[a, j1, b]])
  expect(foldRuns([j1, a])).toEqual([[j1, a]])
  expect(foldRuns([a, j1])).toEqual([[a, j1]])
  expect(foldRuns([j1, think, j2])).toEqual([[j1, think, j2]])
  expect(foldRuns([text('x'), j1, text('y')])).toEqual([text('x'), j1, text('y')])
  // A deliverable splits a run that holds a job.
  const s = tool(9, { name: 'show' })
  expect(foldRuns([a, j1, s, b, c])).toEqual([[a, j1], s, [b, c]])
  expect(foldRuns([a, s, j1, b])).toEqual([a, s, [j1, b]])
  // A contact, subagent or reminder card is never a tool-run member, and a job never joins their group.
  const contact = peer('k', 'contact:e1:sky', 'hi', { origin: { id: 'm1', from: 'contact:e1:sky', reply_to: null, scope: null, contact_mode: null } })
  const kid = peer('s', 'session:kid', 'done', { origin: { id: 'm2', from: 'session:kid', reply_to: null, scope: null, contact_mode: null } })
  const reminder = peer('rm', 'Reminder r3 · after 10 min', 'ping')
  for (const p of [contact, kid, reminder]) {
    expect(foldRuns([a, p, b])).toEqual([a, p, b])
    expect(foldRuns([p, j1, a])).toEqual([p, [j1, a]])
  }
  // The body never makes a job: only the sender does.
  const looks = peer('l', 'session:kid', 'job j4', { origin: { id: 'm3', from: 'session:kid', reply_to: null, scope: null, contact_mode: null } })
  expect(foldRuns([a, looks, b])).toEqual([a, looks, b])
  // A job removed from context stays its own tombstone.
  const removed = peer('jr', 'job j5', 'out', { context: { target: { kind: 'message', seq: 3 }, active: false, removed: true, remove_with: [], attachments: [] } } as Partial<Peer>)
  expect(foldRuns([a, removed, b])).toEqual([a, removed, b])
})

test('summarize: live status names the current call and its position; the ending summary counts, times and names failures', () => {
  const live = summarize([tool(1, { startedAt: 1000, finishedAt: 1400 }), tool(2, { result: undefined }), tool(3, { pending: true, result: undefined })])
  expect(live).toMatchObject({ text: 'running `t2` · 2 of 3', running: true, failed: false, hint: '✓ out 1  (+1 lines)' })
  const done = summarize([
    tool(1, { startedAt: 1000 }),
    tool(2, { result: { output: 'boom', isError: true, files: [] } }),
    tool(3, { finishedAt: 3100 }),
  ])
  expect(done).toMatchObject({ text: '3 tools · 2.1 s · 1 failed (t2)', running: false, failed: false, hint: '✓ out 3  (+1 lines)' })
  // History rows carry no times: no duration.
  expect(summarize([tool(1), tool(2)]).text).toBe('2 tools')
})

const fail = (n: number): Tool => tool(n, { result: { output: 'boom', isError: true, files: [] } })
/** `n` finished calls, the first `failed` of them failed; numbered from `from`. */
const finished = (n: number, failed: number, from = 1): Tool[] => Array.from({ length: n }, (_, i) => (i < failed ? fail(from + i) : tool(from + i)))

test('summarize: red only when more completed calls failed than succeeded; the header still names every failure', () => {
  expect(summarize(finished(34, 2))).toMatchObject({ text: '34 tools · 2 failed (t1, t2)', failed: false })
  expect(summarize(finished(1, 1)).failed).toBe(true)
  // A tie is not a majority.
  expect(summarize(finished(2, 1)).failed).toBe(false)
  expect(summarize(finished(3, 2)).failed).toBe(true)
  // Running: only completed calls count.
  const pending = tool(99, { result: undefined })
  expect(summarize([...finished(1, 1), pending])).toMatchObject({ running: true, failed: true })
  expect(summarize([...finished(3, 1), pending])).toMatchObject({ running: true, failed: false })
})

test('ToolRunCard: the red border follows the majority, not any failure', () => {
  const render = (run: Tool[]) => renderToStaticMarkup(createElement(ToolRunCard, { run, prompts: null, session: 's' }))
  const few = render(finished(34, 2, 101))
  expect(few).toContain('2 failed (t101, t102)')
  expect(few).not.toContain('border-danger-border')
  expect(render(finished(3, 2, 201))).toContain('border-danger-border')
})

test('ToolRunCard: collapsed shows one header and no cards; expanded shows all three', () => {
  const run = [tool(1), tool(2), tool(3)]
  const render = () => renderToStaticMarkup(createElement(ToolRunCard, { run, prompts: null, session: 's' }))
  const collapsed = render()
  expect(collapsed).toContain('3 tools')
  expect(collapsed).toContain('aria-expanded="false"')
  expect(collapsed).not.toContain('>t1<')
  useToolRuns.setState({ disclosed: { [disclosureKey('run', 'c1')]: true } })
  const expanded = render()
  expect(expanded).toContain('aria-expanded="true"')
  for (const name of ['t1', 't2', 't3']) expect(expanded).toContain(`>${name}<`)
})

test('summarize counts job outputs after tools; a jobs-only run names no tools', () => {
  const job = (n: number) => peer(`j${n}`, `j${n} · make · exit 0 · 3s`, `out ${n}`)
  const think: Entry = { id: 'r', kind: 'reasoning', text: 'hm', done: true }
  expect(summarize([tool(1), tool(2), job(1), tool(3), think, tool(4)]).text).toBe('4 tools · 1 job output')
  expect(summarize([job(1), tool(1, { startedAt: 1000 }), fail(2), job(2)]).text).toBe('2 tools · 2 job outputs · 1 failed (t2)')
  expect(summarize([job(1), job(2)])).toMatchObject({ text: '2 job outputs', running: false, failed: false, hint: null })
  // Live: the position counts calls only.
  expect(summarize([job(1), tool(1, { result: undefined })]).text).toBe('running `t1` · 1 of 1')
})

test('ToolRunCard: a run that starts with a job is a tool run; expanded, the job card shows its raw output', () => {
  const run = [peer('j1', 'job j4', 'raw *output*'), tool(1)]
  const render = () => renderToStaticMarkup(createElement(ToolRunCard, { run, prompts: null, session: 's' }))
  const collapsed = render()
  expect(collapsed).toContain('1 tool · 1 job output')
  expect(collapsed).not.toContain('raw *output*')
  useToolRuns.setState({ disclosed: { [disclosureKey('run', 'j1')]: true } })
  const expanded = render()
  expect(expanded).toContain('raw *output*')
  expect(expanded).toContain('>t1<')
})

const py = (extra: Partial<Tool> = {}): Tool => tool(50, { name: 'python', callId: 'p1', args: { code: 'x = read_file("a")\nprint(x)' }, ...extra })
const kid = (n: number, extra: Partial<Tool> = {}): Tool => tool(60 + n, { callId: `p1.${n}`, child: true, name: `k${n}`, ...extra })

test('foldRuns: a python call and the calls its cell made are one group, folded with neighbouring runs; deliverables it made stay plain after it', () => {
  const [a, b, k1, k2] = [tool(1), tool(2), kid(1), kid(2)]
  const shown = kid(3, { name: 'show' })
  expect(foldRuns([a, b, py(), k1, shown, k2, tool(3), tool(4)])).toEqual([[a, b, py(), k1, k2], shown, [tool(3), tool(4)]])
  // A cell that made no calls is still a group; a restart note keeps it a plain entry.
  expect(foldRuns([py()])).toEqual([[py()]])
  const restarted = py({ restart: { clock: '09:00:00', after: 9, cause: 'restart', replayed: 1, interrupted: 0, text: '' } })
  expect(foldRuns([restarted])).toEqual([restarted])
  // With folding off only the group forms.
  expect(foldRuns([a, b, py(), k1], undefined, false)).toEqual([a, b, [py(), k1]])
})

test('foldRuns: parallel cells (two python calls in one message) each get their own calls, wherever they arrived', () => {
  const p1 = py()
  const p2 = tool(51, { name: 'python', callId: 'p2', args: { code: 'y = grep("b")' } })
  const [a1, b1] = [kid(1), tool(71, { callId: 'p2.1', child: true, name: 'g1' })]
  const a2 = kid(2)
  expect(foldRuns([p1, p2, a1, b1, a2])).toEqual([[p1, a1, a2, p2, b1]])
  expect(summarizeGroup([p1, a1, a2]).text).toBe('2 tools')
  expect(summarizeGroup([p2, b1]).text).toBe('1 tool')
  // A call whose cell is not on screen stays where it is (here a tool call beside the cells, so in their fold).
  const stray = tool(72, { callId: 'gone.1', child: true })
  expect(foldRuns([p1, p2, stray, a1])).toEqual([[p1, a1, p2, stray]])
})

test('foldRuns: consecutive cells that called nothing fold into one `ran code × N` card', () => {
  const cell = (n: number, extra: Partial<Tool> = {}): Tool => tool(80 + n, { name: 'python', callId: `q${n}`, args: { code: 'x = 1' }, startedAt: 0, finishedAt: 300, ...extra })
  const [q1, q2, q3] = [cell(1), cell(2), cell(3)]
  const think = (id: string): Entry => ({ id, kind: 'reasoning', text: 'hm', done: true })
  const [t1, t2] = [think('t1'), think('t2')]
  // Reasoning between two cells rides along; a trailing one stays outside.
  expect(foldRuns([q1, t1, q2, t2, q3])).toEqual([[q1, t1, q2, t2, q3]])
  expect(foldRuns([q1, q2, t1])).toEqual([[q1, q2], t1])
  expect(summarizeCells([q1, t1, q2, t2, q3]).text).toBe('ran code × 3 · 0.9 s')
  // A lone one stays its own group; a pinned thinking block splits the run.
  expect(foldRuns([q1, t1])).toEqual([[q1], t1])
  expect(foldRuns([q1, t1, q2], new Set(['t1']))).toEqual([[q1], t1, [q2]])
  // A cell with calls joins the fold; a failed cell or a running cell ends it.
  expect(foldRuns([q1, q2, py(), kid(1), q3])).toEqual([[q1, q2, py(), kid(1), q3]])
  const failed = cell(4, { result: { output: 'NameError', isError: true, files: [] } })
  expect(foldRuns([q1, failed, q2])).toEqual([[q1], [failed], [q2]])
  const running = cell(5, { result: undefined, pending: false })
  expect(foldRuns([q1, q2, running])).toEqual([[q1, q2], [running]])
  // With folding off cells still fold with each other.
  expect(foldRuns([q1, q2], undefined, false)).toEqual([[q1, q2]])
  // The card: one header, collapsed.
  const html = renderToStaticMarkup(createElement(ToolRunCard, { run: [q1, t1, q2], prompts: null, session: 's' }))
  expect(html).toContain('ran code × 2')
  expect(html).not.toContain('>code<')
})

test('summarizeGroup: counts the cell\'s calls, never says python, shows a failure', () => {
  expect(summarizeGroup([py(), kid(1), kid(2), kid(3)]).text).toBe('3 tools')
  expect(summarizeGroup([py(), kid(1)]).text).toBe('1 tool')
  // A cell that called nothing ran code; it never claims "no tools".
  expect(summarizeGroup([py()]).text).toBe('ran code')
  expect(summarizeGroup([py({ result: undefined }), kid(1), kid(2, { result: undefined })])).toMatchObject({ text: 'running · 2 tools', running: true })
  expect(summarizeGroup([py({ result: undefined })]).text).toBe('running')
  expect(summarizeGroup([py(), kid(1), fail(2)])).toMatchObject({ failed: false })
  const crashed = summarizeGroup([py({ result: { output: 'NameError: x', isError: true, files: [] } }), kid(1)])
  expect(crashed).toMatchObject({ failed: true, hint: '✗ NameError: x' })
  expect(crashed.text).toContain('code failed')
  expect(crashed.text).not.toContain('python')
})

test('PythonGroup: collapsed `N tools`; expanded a collapsed `code` row and the cards; code expanded shows the source', () => {
  useToolRuns.setState({ disclosed: {} })
  const group = [py(), kid(1), kid(2)]
  const render = () => renderToStaticMarkup(createElement(ToolRunCard, { run: group, prompts: null, session: 's' }))
  const collapsed = render()
  expect(collapsed).toContain('2 tools')
  expect(collapsed).not.toContain('>code<')
  expect(collapsed).not.toContain('>k1<')
  useToolRuns.setState({ disclosed: { [disclosureKey('run', 'p1')]: true } })
  const expanded = render()
  expect(expanded).toContain('>code<')
  expect(expanded).toContain('>k1<')
  expect(expanded).toContain('>k2<')
  expect(expanded).not.toContain('read_file(')
  useToolRuns.setState({ disclosed: { [disclosureKey('run', 'p1')]: true, [disclosureKey('code', 'p1')]: true } })
  const code = render()
  expect(code).toContain('read_file')
  // The highlighted source may carry its language as an attribute; the headers never name it.
  for (const html of [collapsed, expanded]) expect(html).not.toMatch(/python/i)
})

test('PythonGroup: a cell error stays visible collapsed', () => {
  useToolRuns.setState({ disclosed: {} })
  const html = renderToStaticMarkup(createElement(ToolRunCard, { run: [py({ result: { output: 'NameError: x', isError: true, files: [] } })], prompts: null, session: 's' }))
  expect(html).toContain('✗ NameError: x')
  expect(html).toContain('border-danger-border')
})

test('foldRuns: a direct set_status call draws nothing and no longer splits a run; a failed one keeps its card (212 §12)', () => {
  const [a, b, c] = [tool(1), tool(2), tool(3)]
  const status = tool(9, { name: 'set_status', args: { status: 'merging' }, result: { output: 'status set', isError: false, files: [] } })
  const running = tool(8, { name: 'set_status', pending: true, result: undefined })
  const failed = tool(7, { name: 'set_status', result: { output: 'set_status: status must not be empty', isError: true, files: [] } })
  expect(foldRuns([a, status, b, c])).toEqual([[a, b, c]])
  expect(foldRuns([a, running, b])).toEqual([[a, b]])
  expect(foldRuns([status])).toEqual([])
  expect(foldRuns([a, failed, b])).toEqual([[a, failed, b]])
  // Inside a python cell it stays in the cell's group.
  const py = tool(5, { name: 'python', callId: 'p' })
  const inner = tool(6, { name: 'set_status', callId: 'p.1' })
  expect(foldRuns([py, inner])).toEqual([[py, inner]])
})

/** A finished cell `id` and the `calls` it made, numbered from `n`. */
const cell = (id: string, n: number, calls: number): Tool[] => [
  tool(n, { name: 'python', callId: id, args: { code: `# ${id}` } }),
  ...Array.from({ length: calls }, (_, i) => tool(n + 1 + i, { callId: `${id}.${i + 1}`, child: true, name: `${id}k${i + 1}` })),
]
const thought = (id: string): Entry => ({ id, kind: 'reasoning', text: 'hm', done: true })

test('foldRuns: Sky 2026-11-23 — consecutive groups and tool runs with only thoughts between them are one `10 tools` card', () => {
  useToolRuns.setState({ disclosed: {} })
  // Thought, [2 tools], [1 tool], thought, [1 tool], thought, [4 tools], [2 tools], text.
  const [ta, tb] = [tool(1), tool(2)]
  const [A, B, C, D] = [cell('A', 10, 1), cell('B', 20, 1), cell('C', 30, 4), cell('D', 40, 2)]
  const [t0, t1, t2] = [thought('t0'), thought('t1'), thought('t2')]
  const said = text('x')
  const entries = [t0, ta, tb, ...A, t1, ...B, t2, ...C, ...D, said]
  const fold = [ta, tb, ...A, t1, ...B, t2, ...C, ...D]
  expect(foldRuns(entries)).toEqual([t0, fold, said])
  // Collapse off: the generic calls stay plain, the cells still fold.
  expect(foldRuns(entries, undefined, false)).toEqual([t0, ta, tb, [...A, t1, ...B, t2, ...C, ...D], said])
  expect(summarize(fold).text).toBe('10 tools')
  const render = () => renderToStaticMarkup(createElement(ToolRunCard, { run: fold, prompts: null, session: 's' }))
  const collapsed = render()
  expect(collapsed.match(/aria-expanded/g)).toHaveLength(1)
  expect(collapsed).toContain('10 tools')
  expect(collapsed).not.toContain('>Ak1<')
  // Expanded under the first entry's key: every cell's `code` row, its calls, the generic calls, in order.
  useToolRuns.setState({ disclosed: { [disclosureKey('run', 'c1')]: true } })
  const expanded = render()
  expect(expanded.match(/>code</g)).toHaveLength(4)
  const order = ['>t1<', '>t2<', '>Ak1<', '>Bk1<', '>Ck1<', '>Ck4<', '>Dk2<'].map((s) => expanded.indexOf(s))
  expect(order.every((at, i) => at >= 0 && (i === 0 || at > order[i - 1]!))).toBe(true)
})

test('foldRuns: a group the user opened keeps the fold it joins open', () => {
  const [A, B] = [cell('A', 10, 1), cell('B', 20, 1)]
  useToolRuns.setState({ disclosed: { [disclosureKey('run', 'B')]: true } })
  const html = renderToStaticMarkup(createElement(ToolRunCard, { run: [...A, ...B], prompts: null, session: 's' }))
  expect(html).toContain('>Bk1<')
  useToolRuns.setState({ disclosed: {} })
})

test('foldRuns: a deliverable, a pinned thought, assistant text, a failed or running cell each end a fold of groups', () => {
  const [A, B] = [cell('A', 10, 2), cell('B', 20, 1)]
  const show = tool(9, { name: 'show' })
  expect(foldRuns([...A, show, ...B])).toEqual([A, show, B])
  const t = thought('t')
  expect(foldRuns([...A, t, ...B], new Set(['t']))).toEqual([A, t, B])
  expect(foldRuns([...A, t, ...B])).toEqual([[...A, t, ...B]])
  expect(foldRuns([...A, text('x'), ...B])).toEqual([A, text('x'), B])
  const [crashed, ...ck] = cell('F', 50, 1)
  const failed = [{ ...crashed!, result: { output: 'NameError', isError: true, files: [] } }, ...ck]
  expect(foldRuns([...A, ...failed, ...B])).toEqual([A, failed, B])
  const [live, ...lk] = cell('R', 60, 1)
  const running = [{ ...live!, result: undefined }, ...lk]
  expect(foldRuns([...A, ...B, ...running])).toEqual([[...A, ...B], running])
  // A deliverable a cell made stays plain after its group and splits the fold too.
  const made = tool(70, { callId: 'A.3', child: true, name: 'show' })
  expect(foldRuns([...A, made, ...B])).toEqual([A, made, B])
})
