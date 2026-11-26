import { expect, test } from 'vitest'
import { contactTarget } from '@/lib/contacts'

test('contactTarget: an invite ticket dials as a ticket, an http(s) URL as a card; surrounding whitespace goes', () => {
  expect(contactTarget(' theuth-contact:1.MFRGGZDFMZTWQ2LK\n')).toEqual({ kind: 'ticket', ticket: 'theuth-contact:1.MFRGGZDFMZTWQ2LK' })
  expect(contactTarget('https://agents.example.com/.well-known/agent-card.json')).toEqual({ kind: 'card_url', url: 'https://agents.example.com/.well-known/agent-card.json' })
  expect(contactTarget('http://192.168.8.40:8080/.well-known/agent-card.json')).toEqual({ kind: 'card_url', url: 'http://192.168.8.40:8080/.well-known/agent-card.json' })
})

test.each([
  ['a device pairing key', 'https://relay.theuth.io/link#1.abcdef'],
  ['an empty ticket', 'theuth-contact:'],
  ['plain text', 'hello world'],
  ['another scheme', 'mailto:bob@example.com'],
  ['a Wi-Fi QR', 'WIFI:S:home;T:WPA;P:secret;;'],
  ['nothing', ''],
])('contactTarget: %s is not a contact code', (_label, raw) => {
  expect(contactTarget(raw)).toBeNull()
})
