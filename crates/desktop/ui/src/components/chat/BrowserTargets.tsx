// The header's globe: every browser a chat has — the agent's headless browser (`browser_*`, the `browser` tile /
// `BrowserLive`) and the pages its `show_web` calls opened (205, `web:<id>` leaves) — behind one button.

import { Globe } from 'lucide-react'
import { useMemo, useState } from 'react'
import { Button } from '@/components/ui/button'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { type WebView, useWebViews } from '@/lib/web-views'
import { type Entry, useEntries } from '@/store/sessions'
import { MenuRow, useHeaderMenuClose } from './HeaderMenu'
import { BrowserIndicator } from './Indicators'
import { openWebView, webCall } from './WebViewCard'

export type BrowserTarget = { kind: 'agent' } | { kind: 'web'; id: string; url: string; title: string }

/** `example.com:3000` of `url`, or `url` itself when it does not parse. */
export function hostOf(url: string): string {
  try {
    return new URL(url).host || url
  } catch {
    return url
  }
}

/**
 * The chat's browsers: the agent's (`agent`: one is running and this engine serves it) first, then one per web view
 * frame id, in the order the transcript's `show_web` calls first opened them. A closed tile is gone from the store
 * (`useWebViews.close`) but its call's receipt keeps the id, so it stays re-openable; a frame the store knows for
 * this chat but the transcript does not (yet) list comes last. The store's live address and title win over the call's.
 */
export function browserTargets(agent: boolean, entries: readonly Entry[], views: Record<string, WebView>, session: string): BrowserTarget[] {
  const web = new Map<string, Extract<BrowserTarget, { kind: 'web' }>>()
  for (const e of entries) {
    if (e.kind !== 'tool' || e.name !== 'show_web') continue
    const call = webCall(e)
    if (call.id !== null) web.set(call.id, { kind: 'web', id: call.id, url: call.url, title: call.title })
  }
  for (const v of Object.values(views)) if (v.session === session && !web.has(v.id)) web.set(v.id, { kind: 'web', id: v.id, url: v.url, title: v.title })
  const out: BrowserTarget[] = agent ? [{ kind: 'agent' }] : []
  for (const t of web.values()) {
    const v = views[t.id]
    out.push(v ? { ...t, url: v.current || t.url, title: v.title || t.title } : t)
  }
  return out
}

export function useBrowserTargets(session: string, agent: boolean): BrowserTarget[] {
  const entries = useEntries(session)
  const views = useWebViews((s) => s.views)
  return useMemo(() => browserTargets(agent, entries, views, session), [agent, entries, views, session])
}

/** A target's menu label and its tooltip/aria-label when it is the only one. */
export const targetLabel = (t: BrowserTarget): string => (t.kind === 'agent' ? "Agent's browser (live)" : t.title || hostOf(t.url) || 'Web page')
export const targetAction = (t: BrowserTarget): string => (t.kind === 'agent' ? "Open agent's browser" : `Open ${hostOf(t.url) || t.title || 'web page'}`)

export type OpenContext = {
  session: string
  /** The pane leaf (`useTileHost`), or `null` where nothing tiles. */
  host: string | null
  /** A phone whose shell has native web views: a page opens full screen (209 §7). */
  full: boolean
  /** The agent's browser: its tile, or the `BrowserLive` dialog. */
  openAgent(): void
  /** A page where nothing tiles and no native full screen exists: its `/web/$id` route. */
  route(t: Extract<BrowserTarget, { kind: 'web' }>): void
}

/** Open `t` as the transcript card's Open would: a tile right of the chat (or its pop-out window, focused), full screen
 * on a phone with native views, else the `/web/$id` route; the agent's browser as before. */
export function openTarget(t: BrowserTarget, ctx: OpenContext): void {
  if (t.kind === 'agent') return ctx.openAgent()
  const view = { id: t.id, url: t.url, title: t.title }
  if (ctx.host !== null) return openWebView(ctx.session, view)
  if (ctx.full) return openWebView(ctx.session, view, true)
  ctx.route(t)
}

function TargetIcon({ t, busy, size = 16 }: { t: BrowserTarget; busy: boolean; size?: number }) {
  return t.kind === 'agent' ? (
    <BrowserIndicator live={busy} size={size} className="text-fg-secondary" />
  ) : (
    <Globe size={size} className="shrink-0 text-fg-secondary" />
  )
}

/**
 * The globe, inline (`row`: the compact menu's line). One target: a click opens it. Several: a click or a right-click
 * lists them; picking one opens it.
 */
export function BrowserButton({ targets, busy, row, onOpen }: { targets: BrowserTarget[]; busy: boolean; row?: boolean; onOpen(t: BrowserTarget): void }) {
  const close = useHeaderMenuClose()
  const [menu, setMenu] = useState(false)
  const first = targets[0]
  if (!first) return null
  const agent = targets.some((t) => t.kind === 'agent')
  const icon = <TargetIcon t={agent ? { kind: 'agent' } : first} busy={busy} size={row ? 16 : 20} />
  if (targets.length === 1) {
    const label = targetAction(first)
    return row ? (
      <MenuRow icon={icon} label={agent ? 'Live browser' : 'Web page'} detail={agent ? undefined : hostOf(first.kind === 'web' ? first.url : '')} aria-label={label} onSelect={() => onOpen(first)} />
    ) : (
      <Button variant="ghost" className="size-9 rounded-lg text-fg-secondary" aria-label={label} title={label} onClick={() => onOpen(first)}>
        {icon}
      </Button>
    )
  }
  const label = `Browsers (${targets.length})`
  return (
    <DropdownMenu open={menu} onOpenChange={setMenu}>
      <DropdownMenuTrigger asChild>
        {row ? (
          <MenuRow icon={icon} label="Browsers" detail={targets.length} aria-label={label} />
        ) : (
          <Button
            variant="ghost"
            className="size-9 rounded-lg text-fg-secondary"
            aria-label={label}
            title={label}
            onContextMenu={(e) => {
              e.preventDefault()
              setMenu(true)
            }}
          >
            {icon}
          </Button>
        )}
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-72">
        {targets.map((t) => (
          <DropdownMenuItem
            key={t.kind === 'agent' ? 'agent' : t.id}
            data-browser-target={t.kind === 'agent' ? 'agent' : t.id}
            className="gap-2"
            onSelect={() => {
              close()
              onOpen(t)
            }}
          >
            <TargetIcon t={t} busy={busy} />
            <span className="flex min-w-0 flex-col">
              <span className="truncate">{targetLabel(t)}</span>
              {t.kind === 'web' && <span className="truncate font-mono text-xs text-fg-tertiary">{hostOf(t.url)}</span>}
            </span>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}
