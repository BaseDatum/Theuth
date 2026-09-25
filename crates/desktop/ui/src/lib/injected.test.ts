import { expect, test } from 'vitest'
import { classify } from './injected'

test('human text is plain', () => {
  expect(classify('hello <b>there</b>')).toBeNull()
  expect(classify('<message from="x">unterminated')).toBeNull()
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

test('context summaries and runtime wrappers do not duplicate existing surfaces', () => {
  expect(classify('<context_summary>\nfolded\n</context_summary>\n<runtime>x</runtime>')).toEqual([])
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
      id: '12',
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
  expect(classify('Wake: task 7. Reason: schedule. Do not switch to another task until this wake is handled.')).toEqual([{ kind: 'wake', id: '7', reason: 'schedule' }])
  expect(classify('Wake: no task. Reason: manual. Do not switch to another task until this wake is handled.')).toEqual([{ kind: 'wake', reason: 'manual' }])
})

test('wake accepts every reason and a title with periods', () => {
  for (const reason of ['assigned', 'comment', 'mention', 'schedule', 'answer', 'review', 'resume', 'resumed', 'manual']) {
    expect(classify(`Wake: task a1 — Ship v2.1. Really. Reason: ${reason}. Do not switch to another task until this wake is handled.`)).toEqual([
      { kind: 'wake', id: 'a1', title: 'Ship v2.1. Really', reason },
    ])
  }
})

test('desk opening followed by a wake is two cards', () => {
  expect(classify(`${DESK}\n\n${WAKE_HEAD}\n\n${WAKE_TASK}`)?.map((c) => c.kind)).toEqual(['desk', 'wake'])
})

test('task attributes are taken verbatim and the record body may hold tags and quotes', () => {
  const body = 'Say "hi" to <everyone> & log it.'
  expect(classify(`Wake: task x. Reason: review. Do not switch to another task until this wake is handled.\n\n<task id="x" state="review" owner="user" thread="a &amp; b">\n${body}\n</task>`)).toEqual([
    { kind: 'wake', id: 'x', reason: 'review', state: 'review', owner: 'user', thread: 'a &amp; b', body },
  ])
})

test('a human line starting with Wake: stays a user message', () => {
  expect(classify('Wake: me up at seven')).toBeNull()
  expect(classify(`${DESK}\n\nWake: me up`)).toBeNull()
})
