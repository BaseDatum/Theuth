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
})

test('context summaries and runtime wrappers do not duplicate existing surfaces', () => {
  expect(classify('<context_summary>\nfolded\n</context_summary>\n<runtime>x</runtime>')).toEqual([])
})

test('todo nudges compose with harness blocks but never swallow surrounding human text', () => {
  expect(classify('<todo>x</todo>\n<todo_continue>\nContinue working\n</todo_continue>')).toEqual([
    { kind: 'harness', notice: 'todo', text: 'x' },
    { kind: 'harness', notice: 'todo_continue', text: 'Continue working' },
  ])
  expect(classify('Explain this: <todo_continue>work</todo_continue>')).toBeNull()
  expect(classify('<todo_continue>work</todo_continue> is what I saw')).toBeNull()
  expect(classify('<todo_continue>unterminated')).toBeNull()
})

test('harness notices compose without losing their type or literal body', () => {
  const notices = ['rounds', 'context_pressure', 'recovery', 'continue', 'steer', 'surface_error', 'todo', 'knowledge']
  const body = 'Check <button> &lt;literal&gt; &amp; text.'
  expect(classify(notices.map((notice) => `<${notice}>\n${body}\n</${notice}>`).join('\n'))).toEqual(
    notices.map((notice) => ({ kind: 'harness', notice, text: body })),
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
