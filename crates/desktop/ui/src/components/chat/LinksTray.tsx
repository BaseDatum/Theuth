// The links dash and the Links card: every hyperlink the agent wrote in this chat (`useChatLinks`), one dash at the
// bar's outer right end, one section in the tray above the composer.
import { Globe, Link2, MessageSquareText } from 'lucide-react'
import { useEffect, useRef, useState } from 'react'
import { backgroundColors } from '@/lib/background-colors'
import type { ChatLink } from '@/lib/chat-links'
import { parseTheuthHref } from '@/lib/citations'
import { useScope } from '@/store/engine-scope'
import { useSessionField } from '@/store/sessions'
import { BackgroundCard } from './BackgroundCard'
import { hostOf } from './BrowserTargets'
import { KIND_ICON, KIND_WORD, useOpenChatLink } from './Citations'
import { useTranscriptJump } from './jump'
import { DASH_HIT } from './SubagentStrip'

/** The filter shows above this many links. */
const FILTER_OVER = 8

export const linksCardId = (session: string) => `links-${session}`

/**
 * One dash for all the chat's links, never one per link: a chain link (two overlapping outlined segments). A click opens
 * the tray with the Links card expanded and in view; a new link pulses it once.
 */
export function LinksDash({ session, links }: { session: string; links: ChatLink[] }) {
  const { sessions: store } = useScope()
  const [beat, setBeat] = useState(0)
  const seen = useRef(links.length)
  useEffect(() => {
    if (links.length > seen.current) setBeat((b) => b + 1)
    seen.current = links.length
  }, [links.length])
  if (!links.length) return null
  const label = `${links.length} ${links.length === 1 ? 'link' : 'links'}`
  const open = () => {
    const st = store.getState()
    const already = st.sessions[session]?.todosShown && st.sessions[session]?.linksExpanded
    st.setTodosShown(session, true)
    st.setBackgroundExpanded(session, 'links', true)
    if (already) document.getElementById(linksCardId(session))?.scrollIntoView({ block: 'nearest' })
  }
  return (
    <button type="button" data-links-dash="" title={label} aria-label={`${label} · show links`} className={DASH_HIT} onClick={open}>
      <svg key={beat} width="24" height="8" viewBox="0 0 24 8" aria-hidden className={beat ? 'link-dash link-dash-pulse text-dash-link' : 'link-dash text-dash-link'}>
        <rect x="1" y="1" width="13" height="6" rx="3" fill="none" stroke="currentColor" strokeWidth="2" />
        <rect x="10" y="1" width="13" height="6" rx="3" fill="none" stroke="currentColor" strokeWidth="2" />
      </svg>
    </button>
  )
}

/**
 * The Links section of the tray: newest first, a row per link (favicon, title, host) that opens it as the chat's own link
 * does, and a jump to the reply that wrote it. ~5.5 rows show, the rest scroll; past `FILTER_OVER` a filter appears.
 * `settled`: the tray finished opening — an expanded card then scrolls itself into view.
 */
export function LinksCard({ session, links, settled }: { session: string; links: ChatLink[]; settled: boolean }) {
  const { sessions: store } = useScope()
  const expanded = useSessionField(session, 'linksExpanded') ?? false
  const [query, setQuery] = useState('')
  const ref = useRef<HTMLDivElement>(null)
  const openLink = useOpenChatLink()
  useEffect(() => {
    if (expanded && settled) ref.current?.scrollIntoView({ block: 'nearest' })
  }, [expanded, settled])
  const q = query.trim().toLowerCase()
  const shown = q && links.length > FILTER_OVER ? links.filter((l) => l.text.toLowerCase().includes(q) || l.href.toLowerCase().includes(q)) : links
  return (
    <div ref={ref} id={linksCardId(session)} data-links-card="" className="shrink-0">
      <BackgroundCard
        title={`Links · ${links.length}`}
        color={backgroundColors.links.bg}
        summary={links[0]?.text}
        expanded={expanded}
        onExpanded={(open) => store.getState().setBackgroundExpanded(session, 'links', open)}
        bodyScroll=""
      >
        {links.length > FILTER_OVER && (
          <input
            type="search"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
            placeholder="Filter links"
            aria-label="Filter links"
            className="mb-1 h-7 w-full rounded-md border bg-transparent px-2 text-xs outline-none placeholder:text-fg-tertiary focus-visible:ring-2 focus-visible:ring-ring"
          />
        )}
        {/* 5.5 rows of 36px: the half row says there is more below. */}
        <ul className="max-h-[198px] overflow-y-auto" aria-label="Links">
          {shown.map((link) => <LinkRow key={link.key} session={session} link={link} openLink={openLink} />)}
          {!shown.length && <li role="status" className="py-1 text-fg-tertiary">No link matches.</li>}
        </ul>
      </BackgroundCard>
    </div>
  )
}

function LinkRow({ session, link, openLink }: { session: string; link: ChatLink; openLink(href: string): boolean }) {
  const cite = link.internal ? parseTheuthHref(link.href) : null
  const where = link.internal ? (cite ? KIND_WORD[cite.kind] : 'theuth') : hostOf(link.href)
  return (
    <li className="flex min-h-9 items-center gap-1">
      <a
        href={link.href}
        title={link.href}
        className="flex min-w-0 flex-1 items-center gap-2 rounded py-1 pr-1 outline-none hover:bg-layer-2 focus-visible:ring-2 focus-visible:ring-ring"
        onClick={(e) => {
          if (openLink(link.href)) e.preventDefault()
        }}
      >
        <Favicon link={link} />
        <span className="min-w-0 flex-1 truncate font-medium">{link.text}</span>
        <span className="max-w-[40%] shrink-0 truncate text-fg-tertiary">{where}</span>
      </a>
      <button
        type="button"
        title="Jump to message"
        aria-label={`Jump to the message with ${link.text}`}
        className="flex size-7 shrink-0 items-center justify-center rounded text-fg-tertiary outline-none hover:bg-layer-2 hover:text-foreground focus-visible:ring-2 focus-visible:ring-ring"
        onClick={() => useTranscriptJump.getState().request(session, link.entryId)}
      >
        <MessageSquareText className="size-3.5" />
      </button>
    </li>
  )
}

/** The site's `/favicon.ico`, a neutral globe where it has none; an internal link shows its kind's theuth icon. */
function Favicon({ link }: { link: ChatLink }) {
  const [failed, setFailed] = useState(false)
  const box = 'size-4 shrink-0 text-fg-tertiary'
  if (link.internal) {
    const cite = parseTheuthHref(link.href)
    const Icon = cite ? KIND_ICON[cite.kind] : Link2
    return <Icon aria-hidden className={box} />
  }
  const host = hostOf(link.href)
  if (failed || !host) return <Globe aria-hidden className={box} />
  return <img src={`https://${host}/favicon.ico`} alt="" aria-hidden loading="lazy" referrerPolicy="no-referrer" className="size-4 shrink-0 rounded-sm" onError={() => setFailed(true)} />
}
