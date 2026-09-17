import { expect, test } from 'vitest'
import { domainOf, sourcesFromResult, uniqueDomains, urlsFromResult } from './scrape'

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

const SEARCH = [
  'Searched "rust async": 3 results, 2 pages read, 1.2s',
  '',
  '## 1. Async Book',
  'https://rust-lang.github.io/async-book/',
  'rust-lang.github.io',
  'The async book.',
  '> Futures are lazy.',
  '> They do nothing until polled.',
  '',
  '## 2. Tokio',
  'https://tokio.rs/',
  'tokio.rs',
  'A runtime.',
  '(could not read: timeout)',
  '',
  '## 3. Blog post',
  'https://example.com/post?x=1',
  'example.com · 2024-01-02',
  'Snippet only.',
  '> Excerpt.',
  '',
  'Features:',
  '> not a source',
].join('\n')

test('search_web: header + URL pairs, read when the section has a blockquote', () => {
  expect(sourcesFromResult(ok(SEARCH))).toEqual([
    { url: 'https://rust-lang.github.io/async-book/', title: 'Async Book', read: true },
    { url: 'https://tokio.rs/', title: 'Tokio', read: false },
    { url: 'https://example.com/post?x=1', title: 'Blog post', read: true },
  ])
  expect(urlsFromResult('search_web', ok(SEARCH), { query: 'rust async' })).toEqual([
    'https://rust-lang.github.io/async-book/',
    'https://tokio.rs/',
    'https://example.com/post?x=1',
  ])
})

test('search_web: error result yields nothing', () => {
  expect(sourcesFromResult({ output: SEARCH, isError: true })).toEqual([])
  expect(sourcesFromResult(undefined)).toEqual([])
})

test('search_web: header without a URL line is skipped and closes the previous section', () => {
  const body = ['## 1. Real', 'https://a.example/', '## 2. Orphan', 'no url here', '> stray quote', '## 3. Next', 'https://b.example/'].join('\n')
  expect(sourcesFromResult(ok(body))).toEqual([
    { url: 'https://a.example/', title: 'Real', read: false },
    { url: 'https://b.example/', title: 'Next', read: false },
  ])
})
