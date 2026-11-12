import { useQuerySupport } from '@/rpc/capabilities'
// 144 §6.3–§6.4: `theuth://` links render as numbered chips (ordinal of first appearance in the message), a dashed
// References footer lists them with their resolved title and drift status. One batched `cite_resolve` per message.
// Wave 2 (U1): drift on the chip itself — a `missing`/unavailable target opens the dangling-ref card instead of a
// dead pulse; `changed` says so in the tooltip; inside a notebook chat the route carries `?nb` (§4, the way back).
import { useQuery } from '@tanstack/react-query'
import { type AnyRouter, useRouter } from '@tanstack/react-router'
import { BookOpen, FileText, type LucideIcon, MessageSquare, Mic, StickyNote } from 'lucide-react'
import { type ComponentProps, type ReactNode, createContext, useContext, useEffect, useMemo } from 'react'
import { useNotebookChat } from '@/components/notebooks/NotebookContext'
import { Button } from '@/components/ui/button'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { type Cite, collectCitations, locatorLabel, parseTheuthHref, routeOf } from '@/lib/citations'
import { notebookKeys, notebooksApi } from '@/lib/notebooks-rpc'
import type { Citation } from "@/rpc/bindings/method.agent.output"
import type { CiteKind } from "@/rpc/bindings/method.agent.output"
import { cn } from '@/lib/utils'
import { useConnection } from '@/store/connection'
import { useScopedDisclosure } from './disclosure'

type Resolved = { hrefs: string[]; byHref: Map<string, Citation> }

const Context = createContext<Resolved | null>(null)

const KIND_ICON: Record<CiteKind, LucideIcon> = { notes: StickyNote, sources: FileText, knowledge: BookOpen, meetings: Mic, sessions: MessageSquare }

/** How many distinct citations a message carries. */
export const citationCount = (text: string): number => collectCitations(text).length

/** Collects the message's citations and resolves them once; chips and the footer read the result. */
export function CitationsProvider({ text, children }: { text: string; children: ReactNode }) {
  const canQuery1 = useQuerySupport(["cite_resolve"])
  const hrefs = useMemo(() => collectCitations(text), [text])
  // The connection's client, not `useNotebooksApi()`: a guest page has none and must not throw.
  const client = useConnection((s) => s.client)
  const { data } = useQuery({
    queryKey: notebookKeys.cite(hrefs),
    queryFn: () => notebooksApi(client as NonNullable<typeof client>).cite_resolve(hrefs),
    enabled: canQuery1 && (client !== null && hrefs.length > 0),
    staleTime: 60_000,
  })
  const value = useMemo<Resolved>(() => ({ hrefs, byHref: new Map((data ?? []).map((c) => [c.href, c])) }), [hrefs, data])
  return <Context value={value}>{children}</Context>
}

/**
 * Navigate to a citation's target; a no-op where there is no router (the `/d` guest container). In a notebook chat
 * the route carries `?nb`. `moved` (the id is gone, the text was found elsewhere) opens the document itself: the
 * resolver does not name the new block yet, and the old id would only pulse nothing.
 */
export function useOpenCite(): (cite: Cite, c?: Citation) => void {
  const router = useRouter({ warn: false }) as AnyRouter | undefined
  const nb = useNotebookChat()?.notebook
  return (cite, c) => {
    const route = routeOf(cite, nb)
    void router?.navigate((c?.status === 'moved' ? { ...route, hash: undefined } : route) as never)
  }
}

const dead = (c: Citation | undefined) => c !== undefined && (c.status === 'missing' || !c.available)

const KIND_WORD: Record<CiteKind, string> = { notes: 'note', sources: 'source', knowledge: 'knowledge page', meetings: 'meeting', sessions: 'chat' }

/** The drift word for a tooltip: `changed` = the passage's text differs from when it was cited (§6.4). */
const driftNote = (c: Citation | undefined): string | null =>
  c?.status === 'changed' ? 'source changed since this was cited' : c?.status === 'moved' ? 'passage moved — opens the document' : null

/** Markdown `a`: a `theuth://` href becomes a chip; anything else stays a plain anchor. A dangling chip's card is a
 * disclosure of the message, keyed by the href (171F B13). */
export function CitationLink({ node: _node, href, children, ...rest }: ComponentProps<'a'> & { node?: unknown }) {
  const cite = href ? parseTheuthHref(href) : null
  const ctx = useContext(Context)
  const open = useOpenCite()
  const [shown, setShown] = useScopedDisclosure(`cite|${href ?? ''}`)
  if (!cite) {
    return (
      <a href={href} {...rest}>
        {children}
      </a>
    )
  }
  const n = ctx ? ctx.hrefs.indexOf(cite.href) + 1 : 0
  const c = ctx?.byHref.get(cite.href)
  const drift = driftNote(c)
  const label = [c?.title ?? cite.kind, locatorLabel(cite), drift].filter(Boolean).join(' · ')
  const chip = (
    <a
      href={href}
      className="theuth-cite"
      data-kind={cite.kind}
      data-dead={dead(c) || undefined}
      data-drift={c?.status === 'changed' || c?.status === 'moved' ? c.status : undefined}
      title={label}
      aria-label={`Citation ${n || '•'}: ${label}`}
      onClick={(e) => {
        e.preventDefault()
        if (!dead(c)) open(cite, c)
      }}
    >
      {n || '•'}
    </a>
  )
  if (!dead(c)) return chip
  // §6.4: a dangling target gets the card, not a dead pulse.
  const unavailable = c !== undefined && !c.available
  return (
    <Popover open={shown} onOpenChange={setShown}>
      <PopoverTrigger asChild>{chip}</PopoverTrigger>
      <PopoverContent align="start" className="w-72 p-3 text-xs" data-testid="dangling-ref">
        <p className="font-medium">{unavailable ? `This ${KIND_WORD[cite.kind]} is unavailable` : 'This passage changed or was removed'}</p>
        <p className="mt-1 text-fg-tertiary">{unavailable ? 'It was deleted, or you may not read it.' : `The cited block is no longer in the ${KIND_WORD[cite.kind]}${c?.title ? ` “${c.title}”` : ''}.`}</p>
        {!unavailable && (
          <Button variant="outline" size="xs" className="mt-2" onClick={() => open({ ...cite, locator: null })}>
            Open the {KIND_WORD[cite.kind]}
          </Button>
        )}
      </PopoverContent>
    </Popover>
  )
}

/** The footer under a cited message: `n · kind · title · locator · drift`; nothing without citations. */
export function References() {
  const ctx = useContext(Context)
  const open = useOpenCite()
  if (!ctx || ctx.hrefs.length === 0) return null
  return (
    <div className="theuth-refs mt-2 border-t border-dashed border-border-subtle pt-1.5 text-[11px] text-fg-tertiary">
      {ctx.hrefs.map((href, i) => {
        const cite = parseTheuthHref(href) as Cite
        const c = ctx.byHref.get(href)
        const gone = dead(c)
        const Icon = KIND_ICON[cite.kind]
        const locator = locatorLabel(cite)
        return (
          <button
            type="button"
            key={href}
            className={cn('flex w-full items-baseline gap-1.5 py-px text-left hover:text-fg-secondary', gone && 'opacity-60')}
            onClick={() => (gone ? c?.available && open({ ...cite, locator: null }) : open(cite, c))}
          >
            <span className="theuth-cite" data-kind={cite.kind} data-dead={gone || undefined}>
              {i + 1}
            </span>
            <Icon className="size-3 shrink-0 self-center" aria-label={cite.kind} />
            <span className="truncate">{gone ? `unavailable${c?.title ? ` — ${c.title}` : ''}` : (c?.title ?? `${cite.kind}/${cite.id}`)}</span>
            {locator && <span className="shrink-0 font-mono">{locator}</span>}
            {c && (c.status === 'moved' || c.status === 'changed') && <span className="shrink-0 rounded-full border border-border-subtle px-1.5">{c.status}</span>}
          </button>
        )
      })}
    </div>
  )
}

const PULSE = 'theuth-cite-pulse'

/**
 * `#<block>` in the location: scroll that `.bn-block` into view and pulse it 1.5 s, on mount and on every navigation
 * (the router pushes state, which fires no `hashchange`). `#t<ms>` is a time locator, not a block.
 */
export function usePulseBlock(): void {
  const router = useRouter({ warn: false }) as AnyRouter | undefined
  useEffect(() => {
    if (typeof document === 'undefined') return
    let timer: number | undefined
    let el: Element | null = null
    const run = (tries: number) => {
      clearTimeout(timer)
      el?.classList.remove(PULSE)
      const id = location.hash.slice(1)
      if (!id || /^t\d+$/.test(id)) return
      el = document.querySelector(`.bn-block[data-id="${CSS.escape(id)}"]`)
      if (!el) {
        // The editor may mount before the document's blocks arrive.
        if (tries > 0) timer = window.setTimeout(() => run(tries - 1), 150)
        return
      }
      el.scrollIntoView({ block: 'center' })
      el.classList.add(PULSE)
      timer = window.setTimeout(() => el?.classList.remove(PULSE), 1500)
    }
    const onChange = () => run(10)
    onChange()
    const unsubscribe = router?.history.subscribe(onChange)
    window.addEventListener('hashchange', onChange)
    return () => {
      clearTimeout(timer)
      el?.classList.remove(PULSE)
      unsubscribe?.()
      window.removeEventListener('hashchange', onChange)
    }
  }, [router])
}
