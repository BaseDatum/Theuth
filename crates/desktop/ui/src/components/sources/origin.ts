// 144 §12.1–§12.2: the words and glyphs a source card and page draw from `Source` / `SourceOrigin`. Pure.
import type { ContextLevel, Source, SourceOrigin } from '@/lib/notebooks-types'

export const LEVELS: readonly ContextLevel[] = ['off', 'mention', 'summary', 'full']

/** The 18 px kind glyph of the mock's `.ico`: its text and background class. */
export function originGlyph(origin: SourceOrigin): { text: string; className: string } {
  switch (origin.kind) {
    case 'file':
      return origin.ext.toLowerCase() === 'pdf' ? { text: 'PDF', className: 'bg-orange-700' } : { text: 'FILE', className: 'bg-zinc-500' }
    case 'url':
      return { text: 'WEB', className: 'bg-blue-700' }
    case 'meeting':
      return { text: 'MTG', className: 'bg-emerald-700' }
    case 'text':
      return { text: 'TXT', className: 'bg-zinc-500' }
    case 'session':
      return { text: 'CHAT', className: 'bg-zinc-500' }
    case 'mcp':
      return origin.label === 'jira' ? { text: 'JIRA', className: 'bg-blue-700' } : origin.label === 'confluence' ? { text: 'CONF', className: 'bg-blue-700' } : { text: 'MCP', className: 'bg-zinc-500' }
    case 'youtube':
      return { text: 'YT', className: 'bg-red-700' }
    case 'audio':
      return { text: 'AUD', className: 'bg-purple-700' }
    case 'video':
      return { text: 'VID', className: 'bg-purple-700' }
  }
}

/** The origin word that leads the origin line. */
export function originLabel(origin: SourceOrigin): string {
  switch (origin.kind) {
    case 'file':
      return 'file'
    case 'url':
      return 'url'
    case 'meeting':
      return 'meeting'
    case 'text':
      return 'pasted text'
    case 'session':
      return 'past session'
    case 'mcp':
      return origin.label === 'other' ? 'mcp' : origin.label
    case 'youtube':
      return 'youtube'
    case 'audio':
      return 'audio'
    case 'video':
      return 'video'
  }
}

/** The origin spelled out for the Details tab: file name, url, meeting id, … */
export function originDetail(origin: SourceOrigin): string {
  switch (origin.kind) {
    case 'file':
    case 'audio':
    case 'video':
      return origin.name
    case 'url':
      return origin.canonical ?? origin.url
    case 'meeting':
      return `meeting ${origin.meeting}`
    case 'text':
      return origin.title
    case 'session':
      return `session ${origin.session}`
    case 'mcp':
      return `${origin.server} · ${origin.tool} · ${JSON.stringify(origin.args)}`
    case 'youtube':
      return origin.url
  }
}

/** `just now`, `9 min ago`, `2 h ago`, `3 d ago`. */
export function agoText(ms: number, now = Date.now()): string {
  const s = Math.max(0, (now - ms) / 1000)
  if (s < 60) return 'just now'
  if (s < 3600) return `${Math.floor(s / 60)} min ago`
  if (s < 86400) return `${Math.floor(s / 3600)} h ago`
  return `${Math.floor(s / 86400)} d ago`
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`

/** The card's second line: `file · 48 pages · fetched 3 d ago · 1 insight`, `youtube · 41 min · transcript route: audio → local STT`. */
export function originLine(source: Source, now = Date.now()): string {
  const parts = [originLabel(source.origin)]
  if (source.pages) parts.push(plural(source.pages, 'page'))
  else if (source.duration_ms) parts.push(`${Math.round(Number(source.duration_ms) / 60_000)} min`)
  else if (source.words) parts.push(plural(source.words, 'word'))
  if (source.fetched_ms !== null) parts.push(`fetched ${agoText(Number(source.fetched_ms), now)}`)
  if (source.insights > 0) parts.push(plural(source.insights, 'insight'))
  if (source.origin.kind === 'youtube') parts.push(`transcript route: ${source.origin.route === 'captions' ? 'captions' : 'audio → local STT'}`)
  return parts.join(' · ')
}

const TRANSCRIBED: Partial<Record<SourceOrigin['kind'], true>> = { youtube: true, audio: true, video: true, meeting: true }

export type StatusTone = 'ok' | 'run' | 'fail' | 'muted'

/** The status word at the card's right: `ready`, `transcribing 62 %`, `queued`, `failed · retry`. */
export function statusWord(source: Source): { text: string; tone: StatusTone } {
  switch (source.status) {
    case 'completed':
      return { text: 'ready', tone: 'ok' }
    case 'running': {
      const verb = TRANSCRIBED[source.origin.kind] ? 'transcribing' : 'fetching'
      return { text: source.progress === null ? verb : `${verb} ${source.progress} %`, tone: 'run' }
    }
    case 'failed':
      return { text: 'failed · retry', tone: 'fail' }
    case 'queued':
    case 'new':
      return { text: source.status, tone: 'muted' }
  }
}

/** Audio/video/YouTube, or anything with a duration: shows the scrubber (§12.2). */
export const isMedia = (source: Source): boolean =>
  source.origin.kind === 'youtube' || source.origin.kind === 'audio' || source.origin.kind === 'video' || source.duration_ms !== null

/** `youtube.com/…` or `youtu.be/…`, with or without a scheme and `www.`. */
export const detectUrl = (url: string): 'youtube' | 'url' => (/^(https?:\/\/)?([a-z0-9-]+\.)*(youtube\.com|youtu\.be)\//i.test(url.trim()) ? 'youtube' : 'url')

const MEDIA_EXT: Record<string, 'audio' | 'video'> = {
  m4a: 'audio', mp3: 'audio', wav: 'audio', aac: 'audio', ogg: 'audio', flac: 'audio', opus: 'audio',
  mp4: 'video', mov: 'video', mkv: 'video', webm: 'video', m4v: 'video',
}

export function detectFile(name: string): 'audio' | 'video' | 'file' {
  const i = name.lastIndexOf('.')
  return (i < 0 ? undefined : MEDIA_EXT[name.slice(i + 1).toLowerCase()]) ?? 'file'
}

/** The document's title, else what the origin can say about itself. */
export const sourceTitle = (source: Source): string => source.doc.title || originDetail(source.origin)
