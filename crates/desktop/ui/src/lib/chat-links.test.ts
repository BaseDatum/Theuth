import { expect, test } from 'vitest'
import type { Entry } from '@/store/sessions'
import { collectLinks, extractLinks, normalizeUrl } from './chat-links'

const reply = (id: string, text: string, turn = 1, open = false): Entry => ({ id, kind: 'assistant', text, open, turn })

test('markdown links keep their text; emphasis, code marks and titles are dropped; images are not links', () => {
  expect(extractLinks('See [the **docs**](https://example.com/docs "Docs") and [`api`](<https://example.com/a b>). ![logo](https://example.com/logo.png)')).toEqual([
    { href: 'https://example.com/docs', text: 'the docs' },
    { href: 'https://example.com/a b', text: 'api' },
  ])
  // A linked image is a link (its alt is the text); parens inside the URL balance.
  expect(extractLinks('[![PR](https://img.shields.io/x.svg)](https://github.com/o/r/pull/1) [w](https://en.wikipedia.org/wiki/Foo_(bar))')).toEqual([
    { href: 'https://github.com/o/r/pull/1', text: 'PR' },
    { href: 'https://en.wikipedia.org/wiki/Foo_(bar)', text: 'w' },
  ])
})

test('bare URLs autolink as GFM does: trailing punctuation and an unbalanced paren stay outside; www. gets http://', () => {
  expect(extractLinks('Opened https://github.com/o/r/pull/412. (see https://relay.theuth.io/dash) or www.example.org, <https://a.example>')).toEqual([
    { href: 'https://github.com/o/r/pull/412', text: '' },
    { href: 'https://relay.theuth.io/dash', text: '' },
    { href: 'http://www.example.org', text: '' },
    { href: 'https://a.example', text: '' },
  ])
  // Glued to a word, not a link; a scheme alone is nothing.
  expect(extractLinks('foohttps://x.example and https://')).toEqual([])
})

test('code is never a link: fenced (closed or still open) and inline', () => {
  const text = 'Run `curl https://in.example/line`.\n\n```sh\ncurl https://in.example/fence\n[a](https://in.example/md)\n```\nthen [out](https://out.example/)\n~~~\nhttps://in.example/open-fence'
  expect(extractLinks(text)).toEqual([{ href: 'https://out.example/', text: 'out' }])
})

test('theuth:// counts in markdown and <…> links, never bare (the renderer does not autolink it)', () => {
  expect(extractLinks('[Q3 notes](theuth://notes/n1#b2) <theuth://sessions/s9> theuth://notes/bare')).toEqual([
    { href: 'theuth://notes/n1#b2', text: 'Q3 notes' },
    { href: 'theuth://sessions/s9', text: '' },
  ])
  const [link] = collectLinks([reply('a', '[Q3 notes](theuth://notes/n1#b2) and <theuth://sessions/s9>')])
  expect(link).toMatchObject({ internal: true, text: 'sessions/s9' })
})

test('other schemes and relative links are ignored', () => {
  expect(extractLinks('[mail](mailto:a@b.c) [rel](/docs) [js](javascript:alert(1)) [ftp](ftp://x.example)')).toEqual([])
})

test('streaming: a markdown link counts once its ) arrived, a bare URL once something follows it', () => {
  expect(extractLinks('see [PR](https://github.com/o/r/pu', true)).toEqual([])
  expect(extractLinks('see [PR](https://github.com/o/r/pull/1)', true)).toEqual([{ href: 'https://github.com/o/r/pull/1', text: 'PR' }])
  expect(extractLinks('see https://github.com/o/r/pu', true)).toEqual([])
  // A `.` may yet be `.com`: the run still touches the end.
  expect(extractLinks('see https://github.com.', true)).toEqual([])
  expect(extractLinks('see https://github.com/o/r ', true)).toEqual([{ href: 'https://github.com/o/r', text: '' }])
  // Settled, the same tail counts.
  expect(extractLinks('see https://github.com/o/r/pu')).toEqual([{ href: 'https://github.com/o/r/pu', text: '' }])
  // The open entry in a transcript is extracted the same way.
  expect(collectLinks([reply('a', 'see https://github.com/o/r/pu', 1, true)])).toEqual([])
})

test('dedupe by normalized URL: newest first, latest text and reply win; users and tools are not the agent', () => {
  const entries: Entry[] = [
    reply('a1', 'First [old title](https://GitHub.com/o/r/) then https://relay.example/x', 1),
    { id: 'u2', kind: 'user', text: 'mine: https://user.example' },
    reply('a2', 'Again [PR page](https://github.com/o/r) and https://jira.example/T-88', 2),
    reply('a3', 'Bare https://relay.example/x', 3),
  ]
  const links = collectLinks(entries)
  expect(links.map((l) => [l.key, l.text, l.entryId, l.turn])).toEqual([
    ['https://relay.example/x', 'relay.example/x', 'a3', 3],
    ['https://jira.example/T-88', 'jira.example/T-88', 'a2', 2],
    ['https://github.com/o/r', 'PR page', 'a2', 2],
  ])
  expect(links[2]!.href).toBe('https://github.com/o/r')
})

test('normalizeUrl: host case, default port and a bare trailing slash do not split a link; query and fragment do', () => {
  expect(normalizeUrl('HTTPS://Example.COM:443/')).toBe('https://example.com')
  expect(normalizeUrl('https://example.com/a/')).toBe('https://example.com/a')
  expect(normalizeUrl('https://example.com/a?b=1')).not.toBe(normalizeUrl('https://example.com/a'))
  expect(normalizeUrl('https://example.com/a#x')).not.toBe(normalizeUrl('https://example.com/a'))
  expect(normalizeUrl('not a url')).toBeNull()
})
