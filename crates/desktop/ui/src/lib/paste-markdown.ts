// 165: a rich paste (Teams, Outlook, browsers, Google Docs, Slack, Confluence, Notion…) keeps its links as markdown.
// `pasteText` is the composer's whole decision: clipboard HTML with at least one real link → Turndown markdown;
// anything else → the plain text exactly as before. Redirect wrappers (Safe Links, Google, Slack) are unwrapped to
// the real address on both paths. Pure: Turndown's node build parses with its bundled DOM, the browser's with
// `DOMParser`. Design doc 165-paste-links.
import TurndownService from 'turndown'

/** Redirect wrappers: host, path (`null` = any), the query parameter(s) holding the real address. */
const WRAPPERS: { host: RegExp; path: string | null; params: string[] }[] = [
  { host: /(^|\.)safelinks\.protection\.outlook\.com$/, path: null, params: ['url'] },
  { host: /^(www\.)?google\.[a-z]{2,3}(\.[a-z]{2})?$/, path: '/url', params: ['q', 'url'] },
  { host: /^slack-redir\.net$/, path: '/link', params: ['url'] },
]

const isHttp = (u: string) => /^https?:\/\//i.test(u)

/** The real address behind a redirect wrapper (parameter decoded once, absolute http(s) only), else `href`. */
export function unwrapUrl(href: string): string {
  let url = href
  // A wrapper can wrap another (Safe Links around a Google redirect); three is more than any mail chain does.
  for (let i = 0; i < 3; i++) {
    const next = unwrapOnce(url)
    if (next === url) break
    url = next
  }
  return url
}

function unwrapOnce(href: string): string {
  let u: URL
  try {
    u = new URL(href)
  } catch {
    return href
  }
  const w = WRAPPERS.find((w) => w.host.test(u.hostname.toLowerCase()) && (w.path === null || u.pathname === w.path))
  if (!w) return href
  for (const p of w.params) {
    const v = u.searchParams.get(p)
    if (!v || !isHttp(v)) continue
    try {
      new URL(v)
      return v
    } catch {
      continue
    }
  }
  return href
}

/** Wrapper URLs written out in text (a plain paste, or prose inside the HTML) → their real address. */
function unwrapInText(text: string): string {
  return text.replace(/https?:\/\/[^\s<>"'()[\]]+/g, (m) => {
    const tail = /[.,;:!?]+$/.exec(m)?.[0] ?? ''
    return unwrapUrl(m.slice(0, m.length - tail.length)) + tail
  })
}

/** `www.Example.com/` and `https://example.com` read as the same address. */
const sameAddress = (text: string, url: string) => {
  const norm = (s: string) =>
    s
      .trim()
      .toLowerCase()
      .replace(/^[a-z][a-z0-9+.-]*:(\/\/)?/, '')
      .replace(/^www\./, '')
      .replace(/\/$/, '')
  return norm(text) === norm(url)
}

type El = HTMLElement
const style = (n: El) => (n.getAttribute('style') ?? '').toLowerCase()
const BOLD = /font-weight:\s*(bold|[6-9]00)/
const ITALIC = /font-style:\s*italic/
const inHeading = (n: El) => {
  for (let p = n.parentNode as El | null; p; p = p.parentNode as El | null) if (/^H[1-6]$/.test(p.nodeName)) return true
  return false
}
const tableOf = (n: El) => {
  let p = n.parentNode as El | null
  while (p && p.nodeName !== 'TABLE') p = p.parentNode as El | null
  return p
}
const rowsOf = (table: El) => Array.from(table.querySelectorAll('tr')).filter((r) => tableOf(r) === table)
const cellsOf = (row: El) => Array.from(row.childNodes).filter((c) => c.nodeName === 'TD' || c.nodeName === 'TH')
/** A single-column table is page layout (Outlook mail, signatures): its cells read as paragraphs. */
const isLayout = (table: El | null) => !table || rowsOf(table).every((r) => cellsOf(r).length <= 1)

let service: TurndownService | undefined
let links = 0

function turndown(): TurndownService {
  if (service) return service
  const s = new TurndownService({
    headingStyle: 'atx',
    bulletListMarker: '-',
    codeBlockStyle: 'fenced',
    emDelimiter: '_',
    strongDelimiter: '**',
    br: '',
  })
  // What you'd have typed: no markdown escaping (`file_name_v2`, `1. step`, `*note*`).
  s.escape = (text: string) => text
  s.remove(['style', 'script', 'meta', 'title', 'head', 'noscript'])
  // Word/Outlook: `<o:p>&nbsp;</o:p>` empty-paragraph filler, and the `mso-list:Ignore` bullet/number glyph that
  // only Word reads; a `MsoListParagraph` becomes a list item with that number (`1.`) or `-`.
  s.addRule('msoFiller', {
    filter: (n) => n.nodeName.toLowerCase() === 'o:p' || /mso-list:\s*ignore/.test(style(n)),
    replacement: () => '',
  })
  s.addRule('msoListItem', {
    filter: (n) => n.nodeName === 'P' && /\bMsoListParagraph/.test(n.className),
    replacement: (content, n) => {
      const glyph = ((n as El).querySelector('[style*="mso-list"]')?.textContent ?? '').replace(/\s+/g, ' ').trim()
      return `\n${/^\w{1,3}[.)]$/.test(glyph) ? glyph : '-'} ${content.trim()}\n`
    },
  })
  // `- item` / `1. item`, not Turndown's `-   item`; continuation lines indent to the text.
  s.addRule('listItem', {
    filter: 'li',
    replacement: (content, n) => {
      const parent = n.parentNode as El
      let prefix = '- '
      if (parent.nodeName === 'OL') prefix = `${Number(parent.getAttribute('start') ?? 1) + Array.from(parent.children).indexOf(n)}. `
      const body = content.replace(/^\n+/, '').replace(/\n+$/, /\n$/.test(content) ? '\n' : '')
      return prefix + body.replace(/\n/g, `\n${' '.repeat(prefix.length)}`) + (n.nextSibling ? '\n' : '')
    },
  })
  s.addRule('link', {
    filter: (n) => n.nodeName === 'A' && n.hasAttribute('href'),
    replacement: (content, n) => {
      const text = content.trim().replace(/\s*\n\s*/g, ' ')
      const href = unwrapUrl((n as El).getAttribute('href') ?? '')
      if (!text || !/^(https?|mailto):/i.test(href)) return content
      links++
      if (/^mailto:/i.test(href)) {
        const address = href.slice(7).split('?')[0] ?? ''
        if (sameAddress(text, decodeURIComponent(address)) || sameAddress(text, href)) return text
      } else if (sameAddress(text, href)) return href
      return `[${text}](${href.replace(/ /g, '%20').replace(/\(/g, '%28').replace(/\)/g, '%29')})`
    },
  })
  s.addRule('image', {
    filter: 'img',
    // Emoji (Teams, Slack) carry the character or `:name:` as alt; a longer alt, or a sized picture (a Teams
    // screenshot is `alt="image" width="640"`), is dropped: no remote image links.
    replacement: (_, n) => {
      const el = n as El
      const alt = (el.getAttribute('alt') ?? '').trim()
      const big = Math.max(Number(el.getAttribute('width')) || 0, Number(el.getAttribute('height')) || 0) > 64
      return [...alt].length <= 16 && !big ? alt : ''
    },
  })
  // Google Docs wraps the whole selection in `<b style="font-weight:normal" id="docs-internal-guid-…">`.
  s.addRule('notBold', {
    filter: (n) => (n.nodeName === 'B' || n.nodeName === 'STRONG') && /font-weight:\s*(normal|[1-4]00)/.test(style(n)),
    replacement: (content) => content,
  })
  s.addRule('styledSpan', {
    filter: (n) => n.nodeName === 'SPAN' && (BOLD.test(style(n)) || ITALIC.test(style(n))),
    replacement: (content, n) => {
      if (!content.trim()) return content
      const el = n as El
      let out = ITALIC.test(style(el)) ? `_${content}_` : content
      if (BOLD.test(style(el)) && !inHeading(el)) out = `**${out}**`
      return out
    },
  })
  // A bare `<pre>` (Slack, many pages) is code too; `<pre><code>` stays Turndown's fenced rule.
  s.addRule('barePre', {
    filter: (n) => n.nodeName === 'PRE' && n.firstChild?.nodeName !== 'CODE',
    replacement: (content) => `\n\n\`\`\`\n${content.replace(/^\n+|\n+$/g, '')}\n\`\`\`\n\n`,
  })
  // GFM tables: first row is the header (headerless tables too, unlike turndown-plugin-gfm, which emits raw HTML).
  s.addRule('tableSection', {
    filter: ['thead', 'tbody', 'tfoot'],
    replacement: (content, n) => (isLayout(tableOf(n as El)) ? `\n\n${content}\n\n` : content),
  })
  s.addRule('tableCell', {
    filter: ['th', 'td'],
    replacement: (content, n) => {
      if (isLayout(tableOf(n as El))) return `\n\n${content}\n\n`
      return ` ${content.trim().replace(/\s*\n\s*/g, ' ').replace(/\|/g, '\\|')} |`
    },
  })
  s.addRule('tableRow', {
    filter: 'tr',
    replacement: (content, n) => {
      const table = tableOf(n as El)
      if (isLayout(table) || !table) return `\n\n${content}\n\n`
      const rows = rowsOf(table)
      const width = Math.max(...rows.map((r) => cellsOf(r).length))
      const pad = ' |'.repeat(width - cellsOf(n as El).length)
      const row = `|${content}${pad}`
      return rows[0] === n ? `\n${row}\n|${' --- |'.repeat(width)}\n` : `${row}\n`
    },
  })
  s.addRule('table', {
    filter: 'table',
    replacement: (content) => `\n\n${content}\n\n`,
  })
  service = s
  return s
}

function convert(html: string): { md: string; links: number } {
  links = 0
  const md = turndown()
    .turndown(html)
    // `&nbsp;` is a space, in code too (an nbsp in pasted code breaks it); after conversion, because Turndown
    // moves an element's edge whitespace, nbsp included, outside it unescaped.
    .replace(/\u00a0/g, ' ')
    .replace(/[ \t]+$/gm, '')
    // ponytail: also squeezes blank-line runs inside fenced code; split on fences if that ever matters.
    .replace(/\n{3,}/g, '\n\n')
    .trim()
  return { md: unwrapInText(md), links }
}

/** Clipboard HTML → the markdown you'd have typed. */
export function htmlToMarkdown(html: string): string {
  return convert(html).md
}

/** What a paste inserts: the HTML as markdown when it carries a real link (D2), else `plain`; wrappers unwrapped. */
export function pasteText({ html, plain }: { html: string; plain: string }): string {
  // Cheap gate first: code from an editor carries big HTML and no anchors.
  if (html && /<a\s[^>]*href/i.test(html)) {
    const out = convert(html)
    if (out.links > 0 && out.md) return out.md
  }
  return unwrapInText(plain)
}
