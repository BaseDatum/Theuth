import { expect, test } from 'vitest'
import { effectiveLine, messagingSearch, scopeView, useMessagingSheet, withoutMessaging } from './messaging-sheet'

const state = (over: Partial<Parameters<typeof effectiveLine>[0]> = {}) => ({
  effective: false,
  global: true,
  session: 's-child',
  opted_out: false,
  inherited_from: null,
  ...over,
})

test('188: the effective line ranks the blockers as the engine does: node-wide switch, then an ancestor, then this chat', () => {
  expect(effectiveLine(state({ effective: true }))).toEqual({
    reason: 'A route still needs the peer in scope, and network peers an explicit grant.',
    ancestor: null,
    nodeOff: false,
  })
  // The node-wide switch wins over any opt-out, and links to where it lives instead of offering the ancestor.
  expect(effectiveLine(state({ global: false, opted_out: true, inherited_from: 's-root' }))).toEqual({
    reason: 'Blocked: the node-wide switch is off.',
    ancestor: null,
    nodeOff: true,
  })
  expect(effectiveLine(state({ inherited_from: 's-root' }))).toEqual({
    reason: 'Blocked: an ancestor session opted out (s-root).',
    ancestor: 's-root',
    nodeOff: false,
  })
  // The chat's own opt-out names itself as `inherited_from`: that is not an ancestor.
  expect(effectiveLine(state({ opted_out: true, inherited_from: 's-child' }))).toEqual({ reason: 'Blocked: this chat opted out.', ancestor: null, nodeOff: false })
  expect(effectiveLine(state()).reason).toBe('Blocked.')
})

test('188: root scope is editable only on a loaded top-level chat; a child inherits it with its parent to open', () => {
  expect(scopeView({ loaded: true, parent: null })).toEqual({ kind: 'root' })
  expect(scopeView({ loaded: true, parent: 's-root' })).toEqual({
    kind: 'inherited',
    parent: 's-root',
    note: 'Inherited from the root of this tree; change it from the root session.',
  })
  // Not loaded yet: never offer the root's controls on a guess.
  for (const slice of [undefined, { loaded: false, parent: null }])
    expect(scopeView(slice)).toEqual({
      kind: 'inherited',
      parent: null,
      note: 'Open the chat once to know whether it is a root before editing its scope.',
    })
})

test('188: ?messaging=1 opens the sheet, &peer= focuses a request only alongside it, and both leave the URL afterwards', () => {
  expect(messagingSearch({ messaging: '1', peer: 'm-7' })).toEqual({ messaging: '1', peer: 'm-7' })
  // The router parses `?messaging=1` as a number.
  expect(messagingSearch({ messaging: 1 })).toEqual({ messaging: '1' })
  expect(messagingSearch({ messaging: '1', peer: '' })).toEqual({ messaging: '1' })
  expect(messagingSearch({ messaging: '1', peer: 7 })).toEqual({ messaging: '1' })
  expect(messagingSearch({ peer: 'm-7' })).toEqual({})
  expect(messagingSearch({ messaging: 'yes', peer: 'm-7' })).toEqual({})
  expect(withoutMessaging({ seq: 4, cell: 'plot', messaging: '1' as const, peer: 'm-7' })).toEqual({ seq: 4, cell: 'plot' })
})

test('188: the sheet store opens one chat at a time and forgets the focused request when it closes', () => {
  const sheet = useMessagingSheet.getState()
  sheet.show('s-a', 'm-1')
  expect(useMessagingSheet.getState()).toMatchObject({ session: 's-a', peer: 'm-1' })
  sheet.show('s-b')
  expect(useMessagingSheet.getState()).toMatchObject({ session: 's-b', peer: null })
  sheet.hide()
  expect(useMessagingSheet.getState()).toMatchObject({ session: null, peer: null })
})
