import { createElement, type ReactElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test, vi } from 'vitest'
import { classify, type Injected } from '@/lib/injected'
import { useToolRuns } from '@/lib/tool-runs'
import { DisclosureScope } from './disclosure'
import { ContactInboxCard, ContactMessageCard, EngineRestartCard, MovedCard, restartLine } from './HarnessCards'

// As `disclosure.test.ts`: the cards' module graph (MessageAssistant) reaches the router and the theme listener.
vi.mock('@tanstack/react-router', () => ({ Link: ({ children }: { children: unknown }) => children, useNavigate: () => () => {} }))
vi.mock('@/lib/theme', () => ({ subscribeDark: () => () => {} }))

const moved = (attrs: string) => classify(`<session_moved ${attrs}/>`)![0] as Extract<Injected, { kind: 'moved' }>
const render = (el: ReactElement) => renderToStaticMarkup(createElement(DisclosureScope, { value: 's' }, el))

test('183 mock panel 4: MovedCard says where, from where and what changed; the agent\'s note folds, never as XML', () => {
  const m = moved(`to="xeus" from="Sky's MacBook" os="Arch Linux" arch="x86_64" host="xeus" cwd="/home/sky/Develop/theuth" at="11:42:07" reason="moved" jobs_left="1" runtime="reset" approvals="ask"`)
  const html = render(createElement(MovedCard, { id: 'e1', ...m }))
  expect(html).toContain('Session moved to xeus · Arch Linux')
  expect(html).toContain('>~/Develop/theuth</code>')
  expect(html).toContain('from Sky&#x27;s MacBook · 11:42 · Python runtime reset · 1 job left on Sky&#x27;s MacBook · approvals kept: Ask')
  expect(html).toContain('What the agent was told')
  expect(html).not.toContain('&lt;session_moved')
  // Opened, the note shows as its fields.
  useToolRuns.setState({ disclosed: { 's|moved': true } })
  const open = render(createElement(MovedCard, { id: 'e1', ...m }))
  expect(open).toMatch(/<dt[^>]*>jobs_left<\/dt><dd[^>]*>1<\/dd>/)
  expect(open).not.toContain('session_moved')
  // W-9: a cwd missing on the taker says so; nothing ran in Python, no jobs stayed, messaging off.
  const missing = render(createElement(MovedCard, { id: 'e2', ...moved('to="xeus" from="mac" cwd="/home/sky" cwd_missing="true" cwd_was="/Users/sky/Develop/theuth" at="09:01:00" jobs_left="0" runtime="none" approvals="off" messaging="off"') }))
  expect(missing).toContain('~/Develop/theuth does not exist on xeus — working in ~')
  expect(missing).toContain('from mac · 09:01 · approvals kept: Off · agent messaging off')
  expect(missing).not.toContain('Python runtime reset')
})

test('183 §12.4: EngineRestartCard reads the resume, never the tag', () => {
  const r = classify(`<engine_restart at="11:42:07" after="4s" kind="transfer" from="Sky's MacBook" to="xeus" os="Arch Linux" cwd="/x" replayed="0" interrupted="1"/>`)![0] as Extract<Injected, { kind: 'engine_restart' }>
  const html = render(createElement(EngineRestartCard, r))
  expect(html).toContain('Resumed on xeus after 4 s · moved from Sky&#x27;s MacBook · 1 call interrupted')
  expect(html).not.toContain('engine_restart')
  // Opened (the reader's state in the store), the card shows the note as its fields; closed, only the summary.
  expect(html).not.toMatch(/<dt/)
  useToolRuns.setState({ disclosed: { 's|restart': true } })
  const open = render(createElement(EngineRestartCard, r))
  expect(open).toMatch(/<dt[^>]*>replayed<\/dt><dd[^>]*>0<\/dd>/)
  expect(open).not.toContain('engine_restart')
  useToolRuns.setState({ disclosed: {} })
  expect(restartLine({ ...r, cause: 'takeover', replayed: 2, interrupted: 0 })).toBe("Resumed on xeus after 4 s · taken over from Sky's MacBook · 2 calls replayed")
  expect(restartLine({ clock: '', after: 75, cause: 'crash', replayed: 0, interrupted: 2 })).toBe('Resumed after 1 min · the engine crashed · 2 calls interrupted')
})

test('183 §12.4: ContactMessageCard names the sender, machine and cap; says it is untrusted and that the contact sees only this answer', () => {
  const m = classify('<contact_message contact="bob" machine="Bob&apos;s MacBook" export="ops-chat" cap="read_only">\nThis message comes from bob, a contact outside your mesh.\nIs the build green?\n</contact_message>')![0] as Extract<Injected, { kind: 'contact_message' }>
  const html = render(createElement(ContactMessageCard, { id: 'e1', ...m }))
  expect(html).toContain('bob')
  expect(html).toContain('· read only')
  expect(html).toContain('Is the build green?')
  expect(html).toContain('from a contact · untrusted input')
  expect(html).toContain('will see only this turn&#x27;s answer')
  expect(html).not.toContain('&lt;contact_message')
  const mutating = render(createElement(ContactMessageCard, { id: 'e2', ...m, cap: 'mutating' }))
  expect(mutating).toContain('· can change things')
  // The contact's words are text: a remote image and a link stay inert.
  const hostile = render(createElement(ContactMessageCard, { id: 'e3', ...m, text: '![x](http://attacker/x.png) [go](http://attacker/go)' }))
  expect(hostile).toContain('![x](http://attacker/x.png) [go](http://attacker/go)')
  expect(hostile).not.toContain('<img')
  expect(hostile).not.toContain('href=')
})

test('183 §12.4: ContactInboxCard says whose inbox it is and the cap every turn runs under', () => {
  const c = classify('<contact_inbox contact="bob" machine="Bob\'s MacBook" export="research" cap="read_only"/>\nMessages in this session come from bob, a contact outside your mesh.')![0] as Extract<Injected, { kind: 'contact_inbox' }>
  const html = render(createElement(ContactInboxCard, { id: 'e1', ...c }))
  expect(html).toContain('Inbox for bob')
  expect(html).toContain('Bob&#x27;s MacBook')
  expect(html).toContain('read only')
  expect(html).toContain('research')
  expect(html).toContain('read only: tools that change things are refused.')
  expect(render(createElement(ContactInboxCard, { id: 'e2', ...c, cap: 'mutating' }))).toContain('can change things: destructive tools are refused.')
  expect(html).not.toContain('&lt;contact_inbox')
})
