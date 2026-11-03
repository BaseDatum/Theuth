import { expect, test } from 'vitest'
import { originLabel, originOf, parseMrkdwn, parseShared, previewText, shownText, stripMarker, stripSnippet } from './channel-origin'

const PERMALINK = 'https://acme.slack.com/archives/C0SRE/p1793368920123456?thread_ts=1793368000.000100&cid=C0SRE'
const HEADER = `[forwarded message from Jane Doe · #general · 2026-10-30 14:02 UTC · ${PERMALINK}]`
const SOURCE = '(channel C0SRE, ts 1793368920.123456, thread_ts 1793368000.000100)'

test('originOf: the typed channel first, else the pre-172 Slack marker line; nothing for anything else', () => {
  const typed = { via: 'telegram', chat: 'group', title: 'Family' } as const
  expect(originOf({ text: '[slack dm]\nhi', channel: typed })).toBe(typed)
  expect(originOf({ text: '[slack dm]\nhi' })).toEqual({ via: 'slack', chat: 'dm' })
  expect(originOf({ text: '[slack channel]\nhi' })).toEqual({ via: 'slack', chat: 'group' })
  expect(originOf({ text: '[slack dm]' })).toEqual({ via: 'slack', chat: 'dm' })
  // No chip: a desktop message, a voice note, text that merely starts like the marker.
  for (const text of ['hello', '[voice note, 4 s] call me', '[slack dm] hi', '[slack dms]\nhi', '[slack]\nhi', ' [slack dm]\nhi', 'see [slack dm]\nx'])
    expect(originOf({ text })).toBeUndefined()
})

test('stripMarker drops exactly one leading marker line', () => {
  expect(stripMarker('[slack dm]\nhi\nthere')).toBe('hi\nthere')
  expect(stripMarker('[slack channel]\n[slack dm]\nhi')).toBe('[slack dm]\nhi')
  expect(stripMarker('[slack dm]')).toBe('')
  expect(stripMarker('[slack dm] hi')).toBe('[slack dm] hi')
  expect(stripMarker('hi\n[slack dm]\nx')).toBe('hi\n[slack dm]\nx')
  expect(stripMarker('[voice note, 3 s] hi')).toBe('[voice note, 3 s] hi')
})

test('originLabel follows 172 D4', () => {
  expect(originLabel({ via: 'slack', chat: 'dm' })).toBe('Slack · DM')
  expect(originLabel({ via: 'slack', chat: 'group', title: 'general' })).toBe('Slack · #general')
  expect(originLabel({ via: 'slack', chat: 'group' })).toBe('Slack · channel')
  expect(originLabel({ via: 'signal', chat: 'dm' })).toBe('Signal')
  expect(originLabel({ via: 'signal', chat: 'dm', from: 'Ana' })).toBe('Signal · from Ana')
  expect(originLabel({ via: 'telegram', chat: 'dm' })).toBe('Telegram · DM')
  expect(originLabel({ via: 'telegram', chat: 'group', title: 'Family' })).toBe('Telegram · Family')
  expect(originLabel({ via: 'telegram', chat: 'group' })).toBe('Telegram · group')
  expect(originLabel({ via: 'slack', chat: 'group', title: 'ops', from: 'Jane' })).toBe('Slack · #ops · from Jane')
})

test('parseShared: the slack crate forward with a comment (channel.rs test fixture)', () => {
  const body = stripMarker(`[slack dm]\ncan you check this?\n${HEADER}\n> deploy is rolling back\n> see <#C0OPS>\n${SOURCE}`)
  expect(parseShared(body)).toEqual([
    { kind: 'text', text: 'can you check this?' },
    {
      kind: 'shared',
      message: { kind: 'forwarded', author: 'Jane Doe', channel: 'general', time: '2026-10-30 14:02 UTC', permalink: PERMALINK, body: 'deploy is rolling back\nsee <#C0OPS>' },
    },
  ])
})

test('parseShared: a forward without a comment, then an attachment line', () => {
  const segs = parseShared(`${HEADER}\n> deploy is rolling back\n${SOURCE}\n[attachment from ci-bot] Build #42 passed`)
  expect(segs.map((s) => s.kind)).toEqual(['shared', 'attachment'])
  expect(segs[1]).toEqual({ kind: 'attachment', from: 'ci-bot', text: 'Build #42 passed' })
  expect(parseShared('[attachment]')).toEqual([{ kind: 'attachment', text: '' }])
})

test('parseShared: linked message with an empty quote line, files, no channel/time/permalink, no source', () => {
  const segs = parseShared('[linked message from <@U0JANE>]\n> a\n>\n> ```\n> x\n> ```\n(files: plan.pdf, log.txt)\nthanks')
  expect(segs).toEqual([
    { kind: 'shared', message: { kind: 'linked', author: '<@U0JANE>', body: 'a\n\n```\nx\n```', files: 'plan.pdf, log.txt' } },
    { kind: 'text', text: 'thanks' },
  ])
})

test('parseShared: anything off-grammar stays text', () => {
  const off = [
    'just words',
    '[forwarded message from ]',
    '[forwarded message from A · not-a-field]',
    '[forwarded message from A · 2026-10-30 14:02 UTC · #general]', // fields out of order
    '[shared message from A]\n> x',
    '> a quote on its own',
    '(channel C1, ts 2)',
  ]
  for (const text of off) expect(parseShared(text)).toEqual([{ kind: 'text', text }])
  // The source line only closes a block; a malformed one after the quote is text.
  expect(parseShared('[forwarded message from A]\n> x\n(channel C1 ts 2)')).toEqual([
    { kind: 'shared', message: { kind: 'forwarded', author: 'A', body: 'x' } },
    { kind: 'text', text: '(channel C1 ts 2)' },
  ])
})

test('shownText drops the marker and only an exact `<from>: ` prefix', () => {
  const ops = { via: 'slack', chat: 'group', title: 'ops', from: 'Jane Doe' } as const
  expect(shownText({ text: '[slack channel]\nJane Doe: is the API back up?', channel: ops })).toBe('is the API back up?')
  expect(shownText({ text: 'Ana: dinner at 8?', channel: { via: 'telegram', chat: 'group', from: 'Ana' } })).toBe('dinner at 8?')
  // Not the exact prefix: another name, no space, not at the start, case differs.
  for (const text of ['Jane: hi', 'Jane Doe:hi', 'ok Jane Doe: hi', 'jane doe: hi'])
    expect(shownText({ text, channel: ops })).toBe(text)
  // No `from` (the owner), no channel (desktop): the text as typed, minus a Slack marker.
  expect(shownText({ text: 'Ana: hi', channel: { via: 'telegram', chat: 'dm' } })).toBe('Ana: hi')
  expect(shownText({ text: '[slack dm]\nAna: hi' })).toBe('Ana: hi')
})

test('parseMrkdwn: mentions as #name/@name text, URLs as links; everything else unchanged', () => {
  expect(parseMrkdwn('see <#C0OPS|ops> and <#C0SRE>, ping <@U0JANE|jane> or <@U0SKY>')).toEqual([{ kind: 'text', text: 'see #ops and #C0SRE, ping @jane or @U0SKY' }])
  expect(parseMrkdwn('docs <https://x.example/a?b=1|the runbook> or <https://y.example>.')).toEqual([
    { kind: 'text', text: 'docs ' },
    { kind: 'link', href: 'https://x.example/a?b=1', text: 'the runbook' },
    { kind: 'text', text: ' or ' },
    { kind: 'link', href: 'https://y.example', text: 'https://y.example' },
    { kind: 'text', text: '.' },
  ])
  expect(parseMrkdwn('<#C1|>')).toEqual([{ kind: 'text', text: '#C1' }])
  for (const text of ['a < b > c', '<!here> <!subteam^S1>', '<mailto:a@b.c|a>', '<#lower>', '[the dashboard](https://grafana.example/d/api)', ''])
    expect(parseMrkdwn(text)).toEqual(text ? [{ kind: 'text', text }] : [])
})

test('previews drop the marker; a marker-only title reads as its label; snippet ranges shift', () => {
  expect(previewText('[slack dm]\nship it')).toBe('ship it')
  expect(previewText('[slack channel]')).toBe('Slack · channel')
  expect(previewText('Deploy plan')).toBe('Deploy plan')
  expect(stripSnippet('[slack dm]\nship it', [{ start: 1, end: 6 }, { start: 11, end: 15 }])).toEqual({ text: 'ship it', ranges: [{ start: 0, end: 4 }] })
  expect(stripSnippet('[slack dm]\nship it', [{ start: 9, end: 13 }])).toEqual({ text: 'ship it', ranges: [{ start: 0, end: 2 }] })
  const cut = { text: '…dm]\nship it', ranges: [{ start: 5, end: 9 }] }
  expect(stripSnippet(cut.text, cut.ranges)).toEqual(cut)
})
