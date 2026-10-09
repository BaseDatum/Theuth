import { expect, test } from 'vitest'
import { classify, parseBroadcast, parseSpoken, splitParentView } from './injected'

test('human text is plain', () => {
  expect(classify('hello <b>there</b>')).toBeNull()
  expect(classify('<message from="x">unterminated')).toBeNull()
})

test('broadcast marker: the human speaking to many sessions, not a wrapper', () => {
  const text = '[broadcast 01a0 · 3 sessions]\nstatus report?\n\n- **now**'
  // Still the user's row for the reducer (a turn opener, editable); the card is chosen at render.
  expect(classify(text)).toBeNull()
  expect(parseBroadcast(text)).toEqual({ id: '01a0', count: 3, text: 'status report?\n\n- **now**' })
  expect(parseBroadcast('[broadcast 01a0 · 1 sessions]')).toEqual({ id: '01a0', count: 1, text: '' })
  // Only the supervisor's exact marker at the start; a quoted or dry-run marker stays text.
  expect(parseBroadcast('see [broadcast 01a0 · 3 sessions]\nx')).toBeNull()
  expect(parseBroadcast('[broadcast … · 2 sessions]\nx')).toBeNull()
  expect(parseBroadcast('[broadcast 01a0 · 3 sessions] x')).toBeNull()
})

test('spoken marker: one talk utterance, still the user row', () => {
  expect(classify('[spoken, 5 s] hello there')).toBeNull()
  expect(parseSpoken('[spoken, 5 s] hello there')).toEqual({ secs: 5, interrupted: false, text: 'hello there' })
  expect(parseSpoken('[spoken, 12 s, interrupted] no stop')).toEqual({ secs: 12, interrupted: true, text: 'no stop' })
  expect(parseSpoken('[voice note, 3 s] x')).toBeNull()
  expect(parseSpoken('hello there')).toBeNull()
})

test('skill injection: chip plus the words after it', () => {
  expect(classify('<skill name="caveman">\nbody\n</skill>\n\nbe brief')).toEqual([
    { kind: 'skill', name: 'caveman', words: 'be brief' },
  ])
  // The body may contain `">` (HTML in a skill's prose); it must not truncate the parse.
  expect(classify('<skill name="ponytail">\n`<input type="date">` over a picker\n</skill>')).toEqual([
    { kind: 'skill', name: 'ponytail', words: '' },
  ])
})

test('context summaries, runtime wrappers and the notebook block do not duplicate existing surfaces', () => {
  expect(classify('<context_summary>\nfolded\n</context_summary>\n<runtime>x</runtime>')).toEqual([])
  expect(classify('<notebook id="nb1" title="DR">\n<description>…</description>\n</notebook>')).toEqual([])
})

test('surface_error notes are hidden, with or without attributes, and still compose with visible notices', () => {
  expect(classify('<surface_error>\nfix the diagram\n</surface_error>')).toEqual([])
  expect(classify('<surface_error final="true">\ngave up\n</surface_error>\n<todo>x</todo>')).toEqual([{ kind: 'engine', notice: 'todo', text: 'x' }])
  expect(classify('Explain <surface_error final="true">x</surface_error>')).toBeNull()
})

test('todo nudges compose with engine blocks but never swallow surrounding human text', () => {
  expect(classify('<todo>x</todo>\n<todo_continue>\nContinue working\n</todo_continue>')).toEqual([
    { kind: 'engine', notice: 'todo', text: 'x' },
    { kind: 'engine', notice: 'todo_continue', text: 'Continue working' },
  ])
  expect(classify('Explain this: <todo_continue>work</todo_continue>')).toBeNull()
  expect(classify('<todo_continue>work</todo_continue> is what I saw')).toBeNull()
  expect(classify('<todo_continue>unterminated')).toBeNull()
})

test('engine notices compose without losing their type or literal body', () => {
  const notices = ['rounds', 'context_pressure', 'recovery', 'continue', 'steer', 'todo', 'knowledge']
  const body = 'Check <button> &lt;literal&gt; &amp; text.'
  expect(classify(notices.map((notice) => `<${notice}>\n${body}\n</${notice}>`).join('\n'))).toEqual(
    notices.map((notice) => ({ kind: 'engine', notice, text: body })),
  )
})

test('notice-like human text and malformed wrappers stay intact', () => {
  expect(classify('Explain <rounds>2 remain</rounds>')).toBeNull()
  expect(classify('<rounds>2 remain</rounds> is what I saw')).toBeNull()
  expect(classify('<rounds>unterminated')).toBeNull()
  expect(classify('<rounds source="user">2 remain</rounds>')).toBeNull()
  expect(classify('<unknown>keep this</unknown>')).toBeNull()
})

test('peer and subagent blocks joined by newlines are one card each', () => {
  expect(
    classify('<message from="parent">\nhi\n</message>\n<subagent_result name="Scout">\ndone\n\n</subagent_result>'),
  ).toEqual([
    { kind: 'peer', from: 'parent', text: 'hi' },
    { kind: 'subagent', name: 'Scout', text: 'done' },
  ])
})

test('job result carries id, unescaped command, exit, secs, pty', () => {
  expect(
    classify('<job_result id="j3" command="echo &quot;a&quot; &amp;&amp; ls" exit="0" secs="4" pty="true">\nout\n</job_result>'),
  ).toEqual([{ kind: 'job', id: 'j3', command: 'echo "a" && ls', exit: '0', secs: '4', pty: true, text: 'out' }])
})

test('job idle carries id, command, optional name, quiet, secs, nudge, user_typed and the screen', () => {
  expect(
    classify('<job_idle id="j4" command="omp" name="omp&lt;1&gt;" quiet="63" secs="912" nudge="1" user_typed="12">\n>>> \ncursor: row 1 col 5\n</job_idle>'),
  ).toEqual([
    { kind: 'job_idle', id: 'j4', command: 'omp', name: 'omp<1>', quiet: '63', secs: '912', nudge: '1', userTyped: '12', text: '>>> \ncursor: row 1 col 5' },
  ])
  expect(classify('<job_idle id="j1" command="python3 -q" quiet="120" secs="200" nudge="2">\n</job_idle>')).toEqual([
    { kind: 'job_idle', id: 'j1', command: 'python3 -q', quiet: '120', secs: '200', nudge: '2', text: '' },
  ])
})

test('schedule fire carries name, at, quiet and the prompt body', () => {
  expect(classify('<schedule name="morning-brief" at="2026-09-16T09:00:00-07:00" quiet="false">\nbrief me\n</schedule>')).toEqual([
    { kind: 'schedule', name: 'morning-brief', at: '2026-09-16T09:00:00-07:00', quiet: false, text: 'brief me' },
  ])
})

test('reminder carries id, after, late and the message', () => {
  expect(classify('<reminder id="r3" after="600" set_at="2026-09-23T10:00:00+02:00" late="false">\ncheck the deploy\n</reminder>')).toEqual([
    { kind: 'reminder', id: 'r3', after: '600', late: false, text: 'check the deploy' },
  ])
  expect(classify('<reminder id="r1" after="5" set_at="x" late="true">\n\n</reminder>')).toEqual([
    { kind: 'reminder', id: 'r1', after: '5', late: true, text: '' },
  ])
})

const MANUAL = 'You are the desk of agent `ops` (Operations). You take wakes, read your inbox with `task_inbox`, then set the disposition with `task_update`.'
const DESK = `<desk agent="ops">\n<charter>\n# Ops\n\nKeep the relay up.\n</charter>\n<journal>\n(first episode: no journal yet)\n</journal>\n${MANUAL}\n</desk>`
const WAKE_HEAD = 'Wake: task 12 — Rotate the relay certificate. Reason: comment. (Raised 2 times.) Do not switch to another task until this wake is handled.'
const WAKE_TASK =
  '<task id="12" state="todo" owner="agent:ops" reviewer="agent:cto" project="relay" where="build-A" harness="omp" thread="doc-7">\nThe wildcard cert expires Friday.\nRenew and reload.\n\nExternal: https://jira/SRE-1\n</task>'

test('desk opening carries agent, charter, journal and manual', () => {
  expect(classify(DESK)).toEqual([{ kind: 'desk', agent: 'ops', charter: '# Ops\n\nKeep the relay up.', journal: '(first episode: no journal yet)', manual: MANUAL }])
})

test('wake carries the task header and record', () => {
  expect(classify(`${WAKE_HEAD}\n\n${WAKE_TASK}`)).toEqual([
    {
      kind: 'wake',
      task: '12',
      title: 'Rotate the relay certificate',
      reason: 'comment',
      count: 2,
      state: 'todo',
      owner: 'agent:ops',
      reviewer: 'agent:cto',
      thread: 'doc-7',
      body: 'The wildcard cert expires Friday.\nRenew and reload.\n\nExternal: https://jira/SRE-1',
    },
  ])
})

test('wake without a loaded task keeps the id or names no task', () => {
  expect(classify('Wake: task 7. Reason: schedule. Do not switch to another task until this wake is handled.')).toEqual([{ kind: 'wake', task: '7', reason: 'schedule' }])
  expect(classify('Wake: no task. Reason: manual. Do not switch to another task until this wake is handled.')).toEqual([{ kind: 'wake', reason: 'manual' }])
})

test('wake accepts every reason (70 DESK-PROTOCOL ones carry underscores) and a title with periods', () => {
  for (const reason of ['assigned', 'reassigned', 'comment', 'mention', 'schedule', 'answer', 'review', 'resume', 'resumed', 'manual', 'blockers_resolved', 'children_completed']) {
    expect(classify(`Wake: task a1 — Ship v2.1. Really. Reason: ${reason}. Do not switch to another task until this wake is handled.`)).toEqual([
      { kind: 'wake', task: 'a1', title: 'Ship v2.1. Really', reason },
    ])
  }
})

const ROSTER = '<roster>\nFit check: hand the task over with `task_reassign` if one of these fits it better than your charter; otherwise work it.\n- `karol` (Infrastructure): k8s, networks\n</roster>'

test('an assigned wake carries the roster fit check, with or without a task record', () => {
  const head = 'Wake: task 12 — Rotate the relay certificate. Reason: assigned. Do not switch to another task until this wake is handled.'
  expect(classify(`${head}\n\n${WAKE_TASK}\n\n${ROSTER}`)).toMatchObject([{ kind: 'wake', task: '12', reason: 'assigned', state: 'todo', roster: ROSTER.slice('<roster>\n'.length, -'\n</roster>'.length) }])
  expect(classify(`${head}\n\n${ROSTER}`)).toMatchObject([{ kind: 'wake', task: '12', reason: 'assigned', roster: expect.stringContaining('Fit check') }])
  expect(classify(`${head}\n\n${WAKE_TASK}`)?.[0]).not.toHaveProperty('roster')
})

test('desk nudges and the journal prompt are engine notices, so they use the nudge card and its setting', () => {
  const nudge = 'Task 12 needs a disposition before this turn can end. Move it — exactly one of: `task_update disposition: done` … One attempt: if the task is unchanged when this turn ends it is set `blocked: "no continuation"` and your manager is woken.'
  expect(classify(`<nudge>\n${nudge}\n</nudge>`)).toEqual([{ kind: 'engine', notice: 'nudge', text: nudge }])
  expect(classify('<journal>\nWrite your journal: Done / Open / Next / Notes, ≤ 8 KiB.\n</journal>')).toEqual([{ kind: 'engine', notice: 'journal', text: 'Write your journal: Done / Open / Next / Notes, ≤ 8 KiB.' }])
  // Bare nudge prose is still a user line: the roster must wrap it.
  expect(classify(nudge)).toBeNull()
})

test('desk opening followed by a wake is two cards', () => {
  expect(classify(`${DESK}\n\n${WAKE_HEAD}\n\n${WAKE_TASK}`)?.map((c) => c.kind)).toEqual(['desk', 'wake'])
})

test('task attributes are taken verbatim and the record body may hold tags and quotes', () => {
  const body = 'Say "hi" to <everyone> & log it.'
  expect(classify(`Wake: task x. Reason: review. Do not switch to another task until this wake is handled.\n\n<task id="x" state="review" owner="user" thread="a &amp; b">\n${body}\n</task>`)).toEqual([
    { kind: 'wake', task: 'x', reason: 'review', state: 'review', owner: 'user', thread: 'a &amp; b', body },
  ])
})

test('a human line starting with Wake: stays a user message', () => {
  expect(classify('Wake: me up at seven')).toBeNull()
  expect(classify(`${DESK}\n\nWake: me up`)).toBeNull()
})

// 152: the real rows (sessions.sqlite, 2026-10-09) and `handoff::render`'s shape.
const RESTORED = `<resume_handoff>
The theuth engine restarted at 14:54:32 while you were working on: "Your full brief is the file /Users/Sky.Moore/.theuth/briefs/2026-10-09/151/ENG.md — read it completely first and follow it exactly. Summary: you build the engine half of theuth design 151 AUTORESEARCH-SETUP in the worktree /Users/Sky.Moore/Develop/theuth-151-eng (branch feat/151-eng, cut from feat/151-c which holds the design doc docs/design/151-autoresearch-setup.md and a compiling contract skel…". Your session with this harness has been restored with its context; the files and the git state are as you left them. Continue exactly where you left off — do not start over and do not redo finished steps; if you have uncommitted work, checkpoint-commit it first. Report the result when you are done.
</resume_handoff>`
const FRESH_HEAD =
  'The theuth engine restarted at 09:12:03 and your previous harness session s-old could not be loaded, so its context could not be restored. You are a fresh process in the same working directory; the worktree is as you left it — read git log and git status first. Keep the uncommitted diff: do not revert, stash or discard it.\n'
const FRESH_FOOT =
  'The full earlier transcript is available with session_read("11111111-1111-1111-1111-111111111111"); read it if you need the exact brief. Continue from the uncommitted state — do not start over and do not redo finished steps; checkpoint-commit first if you have uncommitted work.\n'
const FRESH = `<resume_handoff>\n${FRESH_HEAD}Your original brief: "Fix foo.rs and run the tests."
The prompt you were interrupted in: "also fix bar"
What you had reported last: "Edited foo.rs; tests next."
Uncommitted work in /tmp/x (git status --porcelain, 2 lines):
 M foo.rs
?? bar.rs
Diff since the last commit (git diff --stat, 2 lines):
 foo.rs | 3 ++-
 1 file changed
Messages queued for you while you were away (1):
- from session:p: also fix bar
Open todos (2):
- [in_progress] tests
- [pending] docs
${FRESH_FOOT}</resume_handoff>`
const FRESH_CUT = `<resume_handoff>\n${FRESH_HEAD}What you had reported last: "${'r'.repeat(300)}"
Uncommitted work in /tmp/x (git status --porcelain, 100 lines):
 M f0.rs
 M f1.rs
… [1234 chars cut]
(omitted for the handoff budget: git diff, queued messages, open todos)
${FRESH_FOOT}</resume_handoff>`
const LOST = `<child_lost name="omp-98-commands" session="01a10ca4-60c3-7142-bdba-05cc62f673d0" at="09:37" why="its parent is archived">
omp-98-commands was mid-prompt when the engine restarted at 09:37 and was not respawned: its parent is archived. Resume it with send_message or spawn a fresh child with a RESUME brief.
</child_lost>`

test('resume_handoff restored: the time and the interrupted excerpt, the whole note kept', () => {
  const [card] = classify(RESTORED) ?? []
  expect(card).toMatchObject({ kind: 'resume', restored: true, clock: '14:54:32', sections: [] })
  if (card?.kind !== 'resume') throw new Error('not a resume card')
  expect(card.working?.startsWith('Your full brief is the file')).toBe(true)
  expect(card.working?.endsWith('contract skel…')).toBe(true)
  expect(card.text.startsWith('The theuth engine restarted')).toBe(true)
  expect(card.text.endsWith('you are done.')).toBe(true)
  expect(classify('<resume_handoff>\nThe theuth engine restarted at 01:02:03. Your session with this harness has been restored with its context.\n</resume_handoff>')).toMatchObject([{ kind: 'resume', restored: true, clock: '01:02:03' }])
  expect(classify('<resume_handoff>\nThe theuth engine restarted at 01:02:03. Your session with this harness has been restored with its context.\n</resume_handoff>')?.[0]).not.toHaveProperty('working')
})

test('resume_handoff fresh: header, sections in order with counts, the quotes stripped, footer in the note', () => {
  const [card] = classify(FRESH) ?? []
  expect(card).toMatchObject({ kind: 'resume', restored: false, clock: '09:12:03', previous: 's-old' })
  if (card?.kind !== 'resume') throw new Error('not a resume card')
  expect(card.omitted).toBeUndefined()
  expect(card.sections).toEqual([
    { title: 'Original brief', text: 'Fix foo.rs and run the tests.' },
    { title: 'Interrupted prompt', text: 'also fix bar' },
    { title: 'Last report', text: 'Edited foo.rs; tests next.' },
    { title: 'Uncommitted work', text: ' M foo.rs\n?? bar.rs', count: 2 },
    { title: 'Diff', text: ' foo.rs | 3 ++-\n 1 file changed', count: 2 },
    { title: 'Queued messages', text: '- from session:p: also fix bar', count: 1 },
    { title: 'Open todos', text: '- [in_progress] tests\n- [pending] docs', count: 2 },
    { title: 'Note', text: `${FRESH_HEAD.trim()}\n\n${FRESH_FOOT.trim()}` },
  ])
})

test('resume_handoff fresh with a cut section and omitted names', () => {
  const [card] = classify(FRESH_CUT) ?? []
  if (card?.kind !== 'resume') throw new Error('not a resume card')
  expect(card.omitted).toEqual(['git diff', 'queued messages', 'open todos'])
  expect(card.sections.map((s) => s.title)).toEqual(['Last report', 'Uncommitted work', 'Note'])
  expect(card.sections[0]?.text).toBe('r'.repeat(300))
  expect(card.sections[1]).toEqual({ title: 'Uncommitted work', text: ' M f0.rs\n M f1.rs\n… [1234 chars cut]', count: 100 })
})

test('a malformed resume_handoff is still a card: the text becomes its Note', () => {
  const [card] = classify('<resume_handoff>\nsomething else entirely\n</resume_handoff>') ?? []
  expect(card).toEqual({ kind: 'resume', restored: false, clock: '', sections: [{ title: 'Note', text: 'something else entirely' }], text: 'something else entirely' })
  // Followed by another wrapper: one card per block, as for every wrapper.
  expect(classify(`${RESTORED}\n<subagent_result name="k">\ndone\n</subagent_result>`)?.map((c) => c.kind)).toEqual(['resume', 'subagent'])
})

test('child_lost: attributes and body, one card per block, mixable with other wrappers', () => {
  expect(classify(LOST)).toEqual([
    {
      kind: 'lost',
      name: 'omp-98-commands',
      session: '01a10ca4-60c3-7142-bdba-05cc62f673d0',
      clock: '09:37',
      why: 'its parent is archived',
      text: 'omp-98-commands was mid-prompt when the engine restarted at 09:37 and was not respawned: its parent is archived. Resume it with send_message or spawn a fresh child with a RESUME brief.',
    },
  ])
  expect(classify(`${LOST}\n${LOST.replace('omp-98-commands', 'omp-99')}`)?.map((c) => c.kind === 'lost' && c.name)).toEqual(['omp-98-commands', 'omp-99'])
  expect(classify(`${LOST}\n<subagent_result name="k">\ndone\n</subagent_result>`)?.map((c) => c.kind)).toEqual(['lost', 'subagent'])
  expect(classify('<child_lost session="s" at="1">\nx\n</child_lost>')).toBeNull()
})

test('autoresearch: the study card the engine delivered into its setup chat (151 R1) — slug, event and the fence body; never a bubble', () => {
  const card = '```json-render\n{"root":"r","elements":{}}\n```'
  expect(classify(`<autoresearch slug="sort-speed" event="started">\n${card}\n</autoresearch>`)).toEqual([{ kind: 'autoresearch', slug: 'sort-speed', event: 'started', text: card }])
  expect(classify('<autoresearch slug="a&quot;b" event="failed">\n**did not start** — custody\n</autoresearch>')).toEqual([{ kind: 'autoresearch', slug: 'a"b', event: 'failed', text: '**did not start** — custody' }])
  expect(classify('<autoresearch event="started">\nx\n</autoresearch>')).toBeNull()
})

const VIEW_BLOCKS = [
  '<chat>\n0+40|(earlier history, not summarized; recall searches it)\n40+16|Early weeks: the project was set up.\n',
  '56+8|Sky asked for cards; the worker shipped them.\n64+4|A restart lost one child.\n',
  '68+2|Review found a typo.\n',
  '70+1|Now: hand the 152 task to a child.\n</chat>',
]
const VIEW = VIEW_BLOCKS.join('')

test('splitParentView: the four joined blocks, with and without the stamp after </chat>', () => {
  const brief = '\nYour full brief is …'
  const plain = splitParentView(`${VIEW}${brief}`)
  expect(plain?.lines).toEqual([
    { id: 0, n: 40, text: '(earlier history, not summarized; recall searches it)', earlier: true },
    { id: 40, n: 16, text: 'Early weeks: the project was set up.', earlier: false },
    { id: 56, n: 8, text: 'Sky asked for cards; the worker shipped them.', earlier: false },
    { id: 64, n: 4, text: 'A restart lost one child.', earlier: false },
    { id: 68, n: 2, text: 'Review found a typo.', earlier: false },
    { id: 70, n: 1, text: 'Now: hand the 152 task to a child.', earlier: false },
  ])
  expect(plain?.rest).toBe(brief)
  expect(plain?.bytes).toBe(new TextEncoder().encode(VIEW.slice('<chat>\n'.length, -'</chat>'.length)).length)
  const stamped = splitParentView(`${VIEW}<time now="2026-10-09 15:02 -07:00 (Thu)"/>${brief}`)
  expect(stamped?.lines).toHaveLength(6)
  expect(stamped?.rest).toBe(`<time now="2026-10-09 15:02 -07:00 (Thu)"/>${brief}`)
  // A line's text keeps `|` and everything after the first one.
  expect(splitParentView('<chat>\n3+1|a|b|c\n</chat>')?.lines[0]?.text).toBe('a|b|c')
})

test('splitParentView rejects anything but the exact shape', () => {
  expect(splitParentView('<chat>\nhello there\n</chat>\nbrief')).toBeNull()
  expect(splitParentView('<chat>\n3+1|x\n')).toBeNull()
  expect(splitParentView('<chat>3+1|x</chat>')).toBeNull()
  expect(splitParentView('say <chat>\n3+1|x\n</chat>')).toBeNull()
  expect(splitParentView('plain words')).toBeNull()
})

test('152 D5: the budget notice carries attributes and is still an engine notice card', () => {
  const body = "This session's money budget is spent: 0.25 USD of a 0.01 USD cap. The last model call was refused and the turn ended; further calls are refused until the budget is raised. Finish with what is already written."
  expect(classify(`<budget spent="250000000" cap="10000000" currency="USD">\n${body}\n</budget>`)).toEqual([{ kind: 'engine', notice: 'budget', text: body }])
})
