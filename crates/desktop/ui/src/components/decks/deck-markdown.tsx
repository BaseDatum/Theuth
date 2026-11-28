import { type ComponentProps, useMemo } from 'react'
import { type Components, Streamdown, defaultRehypePlugins } from 'streamdown'
import type { PluggableList } from 'unified'
import { rehypePlugins } from '@/components/chat/MessageAssistant'
import { MarkdownTd } from '@/components/chat/surface/registry'
import { lookupPath } from './DeckSurfaceSection'

const PLACEHOLDER_RE = /\{\{([^}|]+?)(?:\|([^}]*))?\}\}/g

/**
 * Strict Deck image URL policy:
 * - Blocks ALL external `http:` / `https:` / protocol-relative `//` URLs (prevents tracking beacons
 *   and data exfiltration via dynamically interpolated image URLs).
 * - Blocks `svg+xml`, `javascript:`, `file:`, `blob:`, and relative paths.
 * - Allows ONLY inline base64 raster `data:image/(png|jpeg|jpg|gif|webp|avif);base64,...` URIs.
 */
const ALLOWED_DATA_IMAGE_RE = /^data:image\/(?:png|jpeg|jpg|gif|webp|avif);base64,[A-Za-z0-9+/=\s]+$/i
const MAX_DATA_IMAGE_CHARS = 4 * 1024 * 1024

export function isAllowedDeckImageUrl(src: unknown): src is string {
  if (typeof src !== 'string') return false
  const trimmed = src.trim()
  if (!trimmed || trimmed.length > MAX_DATA_IMAGE_CHARS) return false
  return ALLOWED_DATA_IMAGE_RE.test(trimmed)
}

/** Strict allow-list for Deck external links and OpenUI `@OpenUrl` actions. */
export function isSafeDeckExternalUrl(url: unknown): url is string {
  if (typeof url !== 'string') return false
  const trimmed = url.trim()
  if (!trimmed || /[\u0000-\u001f\u007f]/.test(trimmed)) return false
  try {
    const parsed = new URL(trimmed)
    return parsed.protocol === 'http:' || parsed.protocol === 'https:'
  } catch {
    return false
  }
}

function formatMarkdownValue(val: unknown, fallback?: string): string {
  if (val === undefined || val === null) return fallback ?? ''
  if (typeof val === 'string') return val
  if (typeof val === 'number' || typeof val === 'boolean') return String(val)
  try {
    const serialized = JSON.stringify(val)
    return serialized === undefined ? (fallback ?? '') : serialized
  } catch {
    return fallback ?? ''
  }
}

/**
 * Single-pass `{{path|default}}` interpolation for Deck Markdown/Note sections.
 * Prototype-safe via `lookupPath`; never re-interpolates substituted strings.
 */
export function interpolateDeckMarkdown(template: string, data: unknown): string {
  if (!template.includes('{{')) return template
  return template.replace(PLACEHOLDER_RE, (_m, rawPath: string, fallback?: string) => {
    const val = lookupPath(data, rawPath.trim())
    return formatMarkdownValue(val, fallback)
  })
}

function DeckMarkdownImage({
  node: _node,
  src,
  alt,
}: ComponentProps<'img'> & { node?: unknown }) {
  if (!isAllowedDeckImageUrl(src)) {
    return (
      <span
        data-deck-blocked-image
        className="inline-block rounded bg-layer-2 px-1.5 py-0.5 text-[11px] text-fg-tertiary"
      >
        [blocked image{alt ? `: ${alt}` : ''}]
      </span>
    )
  }
  return <img src={src.trim()} alt={alt ?? ''} className="max-w-full rounded-lg" />
}

function DeckMarkdownLink({
  node: _node,
  href,
  children,
}: ComponentProps<'a'> & { node?: unknown }) {
  if (!isSafeDeckExternalUrl(href)) {
    return (
      <span data-deck-blocked-link className="text-fg-tertiary">
        {children}
      </span>
    )
  }
  return (
    <a
      href={href.trim()}
      target="_blank"
      rel="noopener noreferrer"
      className="underline underline-offset-2"
    >
      {children}
    </a>
  )
}

/**
 * Safe rehype pipeline for Deck Markdown:
 * - Omits `defaultRehypePlugins.raw` so raw HTML is NEVER parsed (`skipHtml` drops it).
 * - Preserves `data` protocol on `img.src` so inline base64 raster data URIs reach `DeckMarkdownImage`
 *   while external `http(s)` images are blocked both by harden/sanitize and by `DeckMarkdownImage`.
 */
const sanitizeTuple = defaultRehypePlugins.sanitize as [
  unknown,
  { protocols?: { href?: string[]; src?: string[] } },
]
const hardenTuple = defaultRehypePlugins.harden as
  | [unknown, Record<string, unknown>]
  | undefined

export const DECK_MARKDOWN_REHYPE: PluggableList = rehypePlugins
  .filter((p) => p !== defaultRehypePlugins.raw)
  .map((p) => {
    if (Array.isArray(p) && p[0] === sanitizeTuple[0]) {
      const cfg = (p[1] ?? {}) as { protocols?: { href?: string[]; src?: string[] } }
      return [
        p[0],
        {
          ...cfg,
          protocols: {
            ...cfg.protocols,
            href: ['http', 'https'],
            src: ['data'],
          },
        },
      ]
    }
    if (hardenTuple && Array.isArray(p) && p[0] === hardenTuple[0]) {
      const cfg = (p[1] ?? {}) as Record<string, unknown>
      return [
        p[0],
        {
          ...cfg,
          defaultOrigin: 'https://deck.invalid',
          allowedImagePrefixes: ['data:image/'],
          allowedLinkPrefixes: ['http://', 'https://'],
          allowedProtocols: ['http:', 'https:'],
          allowDataImages: true,
        },
      ]
    }
    return p
  }) as PluggableList

export const deckMarkdownComponents: Components = {
  td: MarkdownTd,
  img: DeckMarkdownImage,
  a: DeckMarkdownLink,
}

/**
 * Restricted static Markdown renderer shared by Deck `note` sections and Deck OpenUI
 * `MarkDownRenderer` components: `mode="static"`, `skipHtml`, no raw HTML parsing,
 * external image blocking, and safe `http(s)`-only links.
 */
export function DeckMarkdown({ children, className }: { children: string; className?: string }) {
  return (
    <Streamdown
      mode="static"
      skipHtml
      rehypePlugins={DECK_MARKDOWN_REHYPE}
      components={deckMarkdownComponents}
      className={
        className ??
        'text-xs leading-relaxed text-fg-secondary [&_a]:underline [&_a]:underline-offset-2 [&_p]:my-1 [&_ul]:my-1 [&_ol]:my-1 [&_pre]:my-1.5 [&_code]:font-mono'
      }
    >
      {children}
    </Streamdown>
  )
}

/** Live-interpolated collaborative Markdown note section for Decks. */
export function DeckNoteSection({ content, data }: { content: string; data: unknown }) {
  const rendered = useMemo(() => interpolateDeckMarkdown(content, data), [content, data])
  return (
    <div data-deck-note-markdown>
      <DeckMarkdown>{rendered}</DeckMarkdown>
    </div>
  )
}
