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

const ok = (output: string) => ({ output, isError: false })

test('crawl: page headers and bare link lines, not URLs inside markdown', () => {
  const body = [
    '# https://seed.example/docs/a',
    'Title A',
    'See [other](https://other.example/link) and https://other.example/inline.',
    '# https://seed.example/docs/b.',
    'https://seed.example/docs/c',
    'https://seed.example/docs/a',
  ].join('\n')
  expect(urlsFromResult('crawl', ok(body), { url: 'https://seed.example/docs' })).toEqual([
    'https://seed.example/docs/a',
    'https://seed.example/docs/b',
    'https://seed.example/docs/c',
  ])
})

test('map_site: first token of each line', () => {
  expect(urlsFromResult('map_site', ok('https://a.example/  Home\nhttps://a.example/about  About us\nhttps://a.example/blog'), {})).toEqual([
    'https://a.example/',
    'https://a.example/about',
    'https://a.example/blog',
  ])
})

test('scrape_url: args.url, never the markdown', () => {
  expect(urlsFromResult('scrape_url', ok('# Hi\n\nhttps://other.example/x'), { url: 'https://seed.example/p' })).toEqual(['https://seed.example/p'])
  expect(urlsFromResult('scrape_url', ok('x'), { url: 'file:///etc/passwd' })).toEqual([])
})

test('error or pending result yields nothing', () => {
  expect(urlsFromResult('scrape_url', { output: 'fetch failed', isError: true }, { url: 'https://seed.example/p' })).toEqual([])
  expect(urlsFromResult('crawl', undefined, { url: 'https://seed.example' })).toEqual([])
})
