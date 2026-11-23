// 212 §4: a `show_file` source file, raw — the numbered gutter, `data-line` rows and marked-range tint of a chat
// excerpt (`chat/surface/Excerpt.tsx`, not mounted here: it needs the chat's contexts), virtualised so a long file
// scrolls, highlighted by the shared shiki (`lib/shiki.ts`) once its grammar loads and plain until then.
import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useVirtualizer } from '@tanstack/react-virtual'
import { diffLines } from 'diff'
import { type CSSProperties, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import type { ThemedToken } from 'shiki'
import { queryKeys, useApi } from '@/lib/rpc'
import { tokensOf } from '@/lib/shiki'
import { cn } from '@/lib/utils'
import type { FileViewChange, FileViewText } from '@/rpc/bindings/method.artifact.output'
import { Unavailable, useQuerySupport } from '@/rpc/capabilities'
import { useConnection } from '@/store/connection'

const ROW_PX = 20

/** Extension or well-known file name (what `FileViewOpened.lang` carries, lowercased) → shiki language. */
const LANGS: Record<string, string> = {
  rs: 'rust', ts: 'typescript', mts: 'typescript', cts: 'typescript', tsx: 'tsx', js: 'javascript', mjs: 'javascript',
  cjs: 'javascript', jsx: 'jsx', py: 'python', pyi: 'python', go: 'go', java: 'java', kt: 'kotlin', kts: 'kotlin',
  swift: 'swift', c: 'c', h: 'c', cc: 'cpp', cpp: 'cpp', cxx: 'cpp', hh: 'cpp', hpp: 'cpp', hxx: 'cpp', m: 'objective-c',
  mm: 'objective-cpp', cs: 'csharp', fs: 'fsharp', rb: 'ruby', php: 'php', pl: 'perl', lua: 'lua', r: 'r', dart: 'dart',
  scala: 'scala', ex: 'elixir', exs: 'elixir', erl: 'erlang', hs: 'haskell', ml: 'ocaml', clj: 'clojure', zig: 'zig',
  nim: 'nim', jl: 'julia', sh: 'shellscript', bash: 'shellscript', zsh: 'shellscript', fish: 'fish', ps1: 'powershell',
  sql: 'sql', html: 'html', htm: 'html', xml: 'xml', svg: 'xml', css: 'css', scss: 'scss', sass: 'sass', less: 'less',
  vue: 'vue', svelte: 'svelte', astro: 'astro', json: 'json', jsonc: 'jsonc', json5: 'json5', jsonl: 'json', yaml: 'yaml',
  yml: 'yaml', toml: 'toml', ini: 'ini', cfg: 'ini', md: 'markdown', mdx: 'mdx', tex: 'latex', proto: 'proto',
  graphql: 'graphql', gql: 'graphql', tf: 'hcl', hcl: 'hcl', nix: 'nix', diff: 'diff', patch: 'diff', vim: 'viml',
  dockerfile: 'docker', containerfile: 'docker', makefile: 'make', mk: 'make', justfile: 'just', cmake: 'cmake',
  gradle: 'groovy', groovy: 'groovy', sol: 'solidity', wgsl: 'wgsl', glsl: 'glsl', typ: 'typst',
}

/** The shiki language for a file: its `lang` (extension or well-known name) else `path`'s; `null` (plain text) when
 * neither names a language this viewer knows. */
export function langOf(lang: string | null | undefined, path = ''): string | null {
  const name = path.split(/[\\/]/).pop()?.toLowerCase() ?? ''
  const ext = name.includes('.') ? name.slice(name.lastIndexOf('.') + 1) : name
  for (const k of [lang?.trim().toLowerCase(), name, ext]) if (k && Object.hasOwn(LANGS, k)) return LANGS[k] as string
  return null
}

/** Lines highlighted at most; beyond them (or past `HIGHLIGHT_CHARS` of text, a minified file) rows stay plain. */
export const HIGHLIGHT_LINES = 5000
export const HIGHLIGHT_CHARS = 400_000

/** How many leading `lines` get highlighted: all of a normal file, else up to the line and character budgets. */
export function highlightedLines(lines: readonly string[]): number {
  let chars = 0
  const most = Math.min(lines.length, HIGHLIGHT_LINES)
  for (let i = 0; i < most; i++) {
    chars += lines[i]!.length + 1
    if (chars > HIGHLIGHT_CHARS) return i
  }
  return most
}

/** `text`'s lines, a final newline closing the last one rather than opening an empty one. */
export const linesOf = (text: string): string[] => (text === '' ? [] : text.replace(/\r?\n$/, '').split(/\r?\n/))

/** The tile header's meta: `path · lang · N lines`, then what the view leaves out. */
export function fileMeta(m: { path: string; lang: string | null; lines: number; highlighted: number; truncated?: boolean; lossy?: boolean; deleted?: boolean }): string {
  const parts = [m.path, ...(m.lang ? [m.lang] : []), `${m.lines.toLocaleString('en-US')} ${m.lines === 1 ? 'line' : 'lines'}`]
  if (m.lang && m.highlighted < m.lines) parts.push(`plain after line ${m.highlighted.toLocaleString('en-US')}`)
  if (m.truncated) parts.push('truncated at 2 MiB')
  if (m.lossy) parts.push('not UTF-8')
  if (m.deleted) parts.push('deleted')
  return parts.join(' · ')
}

/** `file_view_read(session, path)`; it changes when the file does (`useFileFollow`), the tile's Refresh, or the agent
 * showing it again (`App.tsx`), never on focus or reconnect. */
export function useFileView(session: string, path: string) {
  const api = useApi()
  const supported = useQuerySupport(['file_view_read'])
  const query = useQuery({
    queryKey: queryKeys.fileView(session, path),
    queryFn: () => api.file_view_read(session, path, null),
    enabled: supported,
    staleTime: Infinity,
    refetchOnWindowFocus: false,
    refetchOnReconnect: false,
  })
  return { ...query, supported }
}

/** Fallback poll of an engine without `subscribe_file_view` (212 §6). */
export const POLL_MS = 3000

/**
 * Follow `path` on disk while mounted (212 §6): `subscribe_file_view` reports each settled change, which refetches the
 * file; on an engine without it, `file_view_read` with `if_mtime_ms` every `POLL_MS`, taking the text only when the
 * file moved on. Returns whether the file is gone (until it reappears).
 */
export function useFileFollow(session: string, path: string): { deleted: boolean } {
  const api = useApi()
  const client = useConnection((s) => s.client)
  const qc = useQueryClient()
  const [deleted, setDeleted] = useState(false)
  useEffect(() => {
    if (!client) return
    const key = queryKeys.fileView(session, path)
    let gone = false
    let timer: ReturnType<typeof setInterval> | undefined
    const poll = async () => {
      const mtime = qc.getQueryData<FileViewText>(key)?.mtime_ms ?? null
      try {
        const next = await api.file_view_read(session, path, mtime)
        if (gone) return
        setDeleted(false)
        if (!next.unchanged) qc.setQueryData(key, next)
      } catch (e) {
        if (!gone && /no such file|not found|does not exist/i.test(e instanceof Error ? e.message : String(e))) setDeleted(true)
      }
    }
    const off = client
      .stream<FileViewChange>('subscribe_file_view', [session, path], 'unsubscribe_file_view', (c) => {
        if (gone) return
        setDeleted(c.deleted)
        if (!c.deleted) void qc.invalidateQueries({ queryKey: key })
      })
      .catch(() => {
        if (!gone) timer = setInterval(() => void poll(), POLL_MS)
        return null
      })
    return () => {
      gone = true
      clearInterval(timer)
      void off.then((stop) => stop?.())
    }
  }, [client, api, qc, session, path])
  return { deleted }
}

/** Lines compared at most (each side) for the changed-lines tint; larger files just take the new text. */
export const COMPARE_LINES = 5000

/** The 1-based lines of `after` that are new against `before` (jsdiff's line diff); `null` past `COMPARE_LINES`. */
export function changedLines(before: string, after: string): Set<number> | null {
  if (linesOf(before).length > COMPARE_LINES || linesOf(after).length > COMPARE_LINES) return null
  const out = new Set<number>()
  let next = 1
  for (const c of diffLines(before, after)) {
    const n = c.count ?? 0
    if (c.added) for (let i = 0; i < n; i++) out.add(next + i)
    if (!c.removed) next += n
  }
  return out
}

/** Where 1-based line `line` of `before` is in `after`: the same text where it survived, else where its run was
 * removed; the scroll anchor across a reload, within `after`'s lines. Past `COMPARE_LINES` the line number stays. */
export function anchorLine(before: string, after: string, line: number): number {
  const total = Math.max(1, linesOf(after).length)
  if (linesOf(before).length > COMPARE_LINES || total > COMPARE_LINES) return Math.min(line, total)
  let old = 1, next = 1
  for (const c of diffLines(before, after)) {
    const n = c.count ?? 0
    if (c.added) { next += n; continue }
    if (line < old + n) return Math.min(c.removed ? next : next + (line - old), total)
    old += n
    if (!c.removed) next += n
  }
  return Math.min(line, total)
}

/** How long changed lines stay tinted after a reload. */
const FLASH_MS = 1500

/** The rows of `text`, scrolled to `line` once on screen; `line..end` tinted. A new `text` (the file changed) keeps
 * the top visible line where it moved to and briefly tints the changed lines. */
function Source({ text, lang, line, end, className }: { text: string; lang: string | null; line?: number; end?: number; className?: string }) {
  const lines = useMemo(() => linesOf(text), [text])
  const lit = useMemo(() => highlightedLines(lines), [lines])
  const [tokens, setTokens] = useState<ThemedToken[][] | null>(null)
  useEffect(() => {
    if (!lang || lit === 0) return
    let live = true
    tokensOf(lines.slice(0, lit).join('\n'), lang).then(
      (rows) => live && setTokens(rows),
      () => undefined, // no grammar: stays plain
    )
    return () => {
      live = false
    }
  }, [lang, lines, lit])
  const box = useRef<HTMLDivElement>(null)
  const v = useVirtualizer({ count: lines.length, getScrollElement: () => box.current, estimateSize: () => ROW_PX, overscan: 20 })
  useEffect(() => {
    if (line) v.scrollToIndex(line - 1, { align: 'center' })
  }, [v, line])
  const shown = useRef(text)
  const [flash, setFlash] = useState<Set<number> | null>(null)
  useLayoutEffect(() => {
    const before = shown.current
    shown.current = text
    const el = box.current
    if (before === text || !el) return
    const top = Math.floor(el.scrollTop / ROW_PX) + 1
    el.scrollTop = (anchorLine(before, text, top) - 1) * ROW_PX + (el.scrollTop % ROW_PX)
    setFlash(changedLines(before, text))
  }, [text])
  useEffect(() => {
    if (!flash) return
    const t = setTimeout(() => setFlash(null), FLASH_MS)
    return () => clearTimeout(t)
  }, [flash])
  const last = end ?? line ?? 0
  const gutter = `${Math.max(3, String(lines.length).length) + 1}ch`
  return (
    <div ref={box} className={cn('min-h-0 overflow-auto font-mono text-xs leading-5', className)} data-file-view="">
      <div className="relative min-w-max" style={{ height: v.getTotalSize() }}>
        {v.getVirtualItems().map((item) => {
          const n = item.index + 1
          // Tokens of the previous text still fit a line whose text is unchanged; any other stays plain until its own arrive.
          const toks = tokens?.[item.index]
          const fits = toks !== undefined && toks.map((t) => t.content).join('') === lines[item.index]
          return (
            <div
              key={item.key}
              data-line={n}
              className={cn(
                'absolute left-0 grid h-5 w-full min-w-max pr-3 transition-colors duration-700',
                line !== undefined && line <= n && n <= last && 'bg-fg-accent/10 shadow-[inset_2px_0_0_0_var(--color-fg-accent)]',
                flash?.has(n) && 'bg-warning-bg',
              )}
              data-changed={flash?.has(n) || undefined}
              style={{ top: item.start, gridTemplateColumns: `${gutter} 14px 1fr` }}
            >
              <span className="text-right text-fg-tertiary tabular-nums select-none" aria-hidden>
                {n}
              </span>
              <span />
              <span className="whitespace-pre">
                {fits
                  ? toks.map((t, j) => (
                      <span key={j} className="shiki" style={t.htmlStyle as CSSProperties}>
                        {t.content}
                      </span>
                    ))
                  : lines[item.index]}
              </span>
            </div>
          )
        })}
      </div>
    </div>
  )
}

/** A `show_file` file (212 §4) fetched with `file_view_read`: a skeleton while loading, `could not load …` on error.
 * `className` sizes it: the tile's full height, or a card's capped box. */
export function FileView({ session, path, lang, line, end, className }: { session: string; path: string; lang?: string | null; line?: number; end?: number; className?: string }) {
  const q = useFileView(session, path)
  if (!q.supported) return <Unavailable />
  if (q.error && !q.data) return <p className={cn('p-3 font-mono text-xs text-danger wrap-anywhere', className)}>could not load {path}: {q.error instanceof Error ? q.error.message : String(q.error)}</p>
  if (!q.data) {
    return (
      <div className={cn('flex flex-col gap-2 p-3', className)} aria-busy="true">
        {[60, 80, 45, 70, 55].map((w) => <div key={w} className="h-3 animate-pulse rounded bg-layer-2" style={{ width: `${w}%` }} />)}
      </div>
    )
  }
  if (!q.data.text) return <div className={cn('p-3 font-mono text-xs text-fg-tertiary', className)}>Empty file</div>
  return <Source text={q.data.text} lang={langOf(q.data.lang || lang, path)} line={line} end={end} className={className} />
}
