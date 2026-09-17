import { expect, test } from 'vitest'
import { domainOf, uniqueDomains, urlsFromResult } from './scrape'

test('domainOf strips www and scheme', () => {
  expect(domainOf('https://www.example.com/docs')).toBe('example.com')
  expect(domainOf('not a url')).toBe('')
})

test('uniqueDomains keeps first-seen host, caps', () => {
  expect(
    uniqueDomains(
      ['https://a.com/1', 'https://www.a.com/2', 'https://b.com/', 'https://c.com/'],
      2,
    ),
  ).toEqual(['a.com', 'b.com'])
})

test('urlsFromResult reads args.url then output links', () => {
  expect(urlsFromResult({ url: 'https://seed.example/docs' }, '# https://seed.example/docs/a\nhttps://seed.example/docs/b  title')).toEqual([
    'https://seed.example/docs',
    'https://seed.example/docs/a',
    'https://seed.example/docs/b',
  ])
})
