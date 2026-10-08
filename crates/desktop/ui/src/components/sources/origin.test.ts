import { expect, test } from 'vitest'
import type { Source, SourceOrigin } from '@/lib/notebooks-types'
import { agoText, detectFile, detectUrl, isMedia, originGlyph, originLabel, originLine, sourceTitle, statusWord } from './origin'

const NOW = Date.UTC(2026, 9, 8, 12)
const DAY = 86_400_000

const source = (origin: SourceOrigin, extra: Partial<Source> = {}): Source => ({
  doc: { id: 'd1', kind: 'source', title: 'T', binding: 'notebook', created_ms: 0n, updated_ms: 0n, bytes: 0n } as unknown as Source['doc'],
  origin,
  status: 'completed',
  progress: null,
  error: null,
  attempts: 1,
  fetched_ms: null,
  content_sha: null,
  changed_since: null,
  bytes: null,
  words: null,
  duration_ms: null,
  pages: null,
  insights: 0,
  meta: null,
  ...extra,
})

const ORIGINS: [SourceOrigin, string, string][] = [
  [{ kind: 'file', name: 'a.pdf', sha256: '', ext: 'pdf' }, 'PDF', 'file'],
  [{ kind: 'file', name: 'a.docx', sha256: '', ext: 'docx' }, 'FILE', 'file'],
  [{ kind: 'url', url: 'https://x', canonical: null }, 'WEB', 'url'],
  [{ kind: 'meeting', meeting: 'm1' }, 'MTG', 'meeting'],
  [{ kind: 'text', title: 'pasted', sha256: '' }, 'TXT', 'pasted text'],
  [{ kind: 'session', session: 's1' }, 'CHAT', 'past session'],
  [{ kind: 'mcp', server: 'jira', tool: 'get_issue', args: {}, label: 'jira' }, 'JIRA', 'jira'],
  [{ kind: 'mcp', server: 'confluence', tool: 'get_page', args: {}, label: 'confluence' }, 'CONF', 'confluence'],
  [{ kind: 'mcp', server: 'x', tool: 'y', args: {}, label: 'other' }, 'MCP', 'mcp'],
  [{ kind: 'youtube', video: 'v', url: 'https://youtu.be/v', route: 'audio' }, 'YT', 'youtube'],
  [{ kind: 'audio', name: 'a.m4a', sha256: '', ext: 'm4a' }, 'AUD', 'audio'],
  [{ kind: 'video', name: 'a.mp4', sha256: '', ext: 'mp4' }, 'VID', 'video'],
]

test('every origin has a glyph and a label', () => {
  for (const [origin, glyph, label] of ORIGINS) {
    expect(originGlyph(origin).text).toBe(glyph)
    expect(originGlyph(origin).className).toMatch(/^bg-/)
    expect(originLabel(origin)).toBe(label)
  }
})

test('the origin line: pages, else minutes, else words; fetched ago; insights; the youtube route', () => {
  const pdf = source({ kind: 'file', name: 'a.pdf', sha256: '', ext: 'pdf' }, { pages: 48, fetched_ms: BigInt(NOW - 3 * DAY), insights: 1, words: 9000 })
  expect(originLine(pdf, NOW)).toBe('file · 48 pages · fetched 3 d ago · 1 insight')
  const yt = source({ kind: 'youtube', video: 'v', url: 'u', route: 'audio' }, { status: 'running', progress: 62, duration_ms: 2_460_000n })
  expect(originLine(yt, NOW)).toBe('youtube · 41 min · transcript route: audio → local STT')
  expect(originLine(source({ kind: 'youtube', video: 'v', url: 'u', route: 'captions' }), NOW)).toBe('youtube · transcript route: captions')
  expect(originLine(source({ kind: 'url', url: 'u', canonical: null }, { fetched_ms: BigInt(NOW - 9 * DAY), insights: 2 }), NOW)).toBe('url · fetched 9 d ago · 2 insights')
  expect(originLine(source({ kind: 'text', title: 't', sha256: '' }, { words: 1 }), NOW)).toBe('pasted text · 1 word')
  expect(originLine(source({ kind: 'session', session: 's' }), NOW)).toBe('past session')
  expect(originLine(source({ kind: 'audio', name: 'a.m4a', sha256: '', ext: 'm4a' }, { duration_ms: 350_000n }), NOW)).toBe('audio · 6 min')
})

test('agoText rounds down to the largest unit', () => {
  expect(agoText(NOW - 10_000, NOW)).toBe('just now')
  expect(agoText(NOW - 9 * 60_000, NOW)).toBe('9 min ago')
  expect(agoText(NOW - 2 * 3_600_000 - 1, NOW)).toBe('2 h ago')
  expect(agoText(NOW - 3 * DAY, NOW)).toBe('3 d ago')
})

test('the status word per status, with the transcribing/fetching verb and the percent', () => {
  const yt: SourceOrigin = { kind: 'youtube', video: 'v', url: 'u', route: 'audio' }
  const url: SourceOrigin = { kind: 'url', url: 'u', canonical: null }
  expect(statusWord(source(yt))).toEqual({ text: 'ready', tone: 'ok' })
  expect(statusWord(source(yt, { status: 'running', progress: 62 }))).toEqual({ text: 'transcribing 62 %', tone: 'run' })
  expect(statusWord(source({ kind: 'meeting', meeting: 'm' }, { status: 'running', progress: null }))).toEqual({ text: 'transcribing', tone: 'run' })
  expect(statusWord(source(url, { status: 'running', progress: 10 }))).toEqual({ text: 'fetching 10 %', tone: 'run' })
  expect(statusWord(source(url, { status: 'running' }))).toEqual({ text: 'fetching', tone: 'run' })
  expect(statusWord(source(url, { status: 'failed', error: 'boom' }))).toEqual({ text: 'failed · retry', tone: 'fail' })
  expect(statusWord(source(url, { status: 'queued' }))).toEqual({ text: 'queued', tone: 'muted' })
  expect(statusWord(source(url, { status: 'new' }))).toEqual({ text: 'new', tone: 'muted' })
})

test('media detection, url/file detection, and the title fallback', () => {
  expect(isMedia(source({ kind: 'youtube', video: 'v', url: 'u', route: 'audio' }))).toBe(true)
  expect(isMedia(source({ kind: 'meeting', meeting: 'm' }, { duration_ms: 1n }))).toBe(true)
  expect(isMedia(source({ kind: 'url', url: 'u', canonical: null }))).toBe(false)
  expect(detectUrl('https://www.youtube.com/watch?v=x')).toBe('youtube')
  expect(detectUrl('youtu.be/x')).toBe('youtube')
  expect(detectUrl('https://notyoutube.com/x')).toBe('url')
  expect(detectUrl('https://docs.aws.amazon.com/')).toBe('url')
  expect(detectFile('memo.M4A')).toBe('audio')
  expect(detectFile('clip.mov')).toBe('video')
  expect(detectFile('runbook.pdf')).toBe('file')
  expect(detectFile('Makefile')).toBe('file')
  expect(sourceTitle(source({ kind: 'url', url: 'https://x', canonical: 'https://x/c' }, { doc: { title: '' } as unknown as Source['doc'] }))).toBe('https://x/c')
  expect(sourceTitle(source({ kind: 'url', url: 'https://x', canonical: null }))).toBe('T')
})
