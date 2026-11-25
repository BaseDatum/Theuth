// The session's panels, moved out of the header to the bar above the composer (thumb reach): terminals, browsers,
// whiteboards, artifacts, the sandbox desktop and the CodeMap. `useTray` gathers what the header used to (same queries,
// same capability gates) with each panel's opener; `BackgroundDashes` draws one dash per item and `TrayTiles` the opened
// tray's grid.

import { useQuery } from '@tanstack/react-query'
import { useNavigate } from '@tanstack/react-router'
import { type ReactNode, useMemo } from 'react'
import { liveRows } from '@/components/agents/sessions'
import { requirements as artifactRequirements } from '@/components/artifacts/requirements'
import { CodeMapButton, useCodemapAvailable } from '@/components/codemap/CodeMapHost'
import { requirements as codemapRequirements } from '@/components/codemap/requirements'
import { useSandboxes } from '@/components/settings/sandbox-queries'
import { useTileHost } from '@/components/tiles/host'
import { openBrowserTile, openTerminalsTile, openVmTile, showArtifact } from '@/components/tiles/open'
import { requirements as vmRequirements } from '@/components/vm/requirements'
import { DropdownMenu, DropdownMenuContent, DropdownMenuItem, DropdownMenuTrigger } from '@/components/ui/dropdown-menu'
import { useIsMobile } from '@/hooks/use-mobile'
import { backgroundColors } from '@/lib/background-colors'
import { canPopout, openPopout, popoutOpen } from '@/lib/popout'
import { queryKeys, useApi } from '@/lib/rpc'
import { nativeWebViews } from '@/lib/web-view-native'
import { useQuerySupport } from '@/rpc/capabilities'
import type { Artifact } from '@/rpc/bindings/method.artifact.output'
import { supports } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'
import { useScope } from '@/store/engine-scope'
import { useBrowserBusy, useSessionField, useSessions } from '@/store/sessions'
import { metaLine } from './ArtifactCard'
import { type BrowserTarget, BrowserButton, hostOf, openTarget, targetLabel, useBrowserTargets } from './BrowserTargets'
import { browserRequirements, terminalRequirements } from './requirements'
import { ActivityDash, SubagentLink, TerminalLink } from './SubagentStrip'
import { TerminalButton, useHasTerminals, useTerminals } from './TerminalViewer'
import { useOpenWhiteboard } from './WhiteboardViewCard'
import { type SessionWhiteboard, useSessionWhiteboards } from '@/components/whiteboard/useSessionWhiteboards'

export type Tray = {
  session: string
  /** A terminal ever ran here (live or retained), on an engine that lists them. */
  terminals: boolean
  browsers: BrowserTarget[]
  browserBusy: boolean
  openBrowser(t: BrowserTarget): void
  /** Newest first. */
  artifacts: Artifact[]
  /** 218: the whiteboards this chat created, showed or is bound to (`useSessionWhiteboards`), and their opener. */
  whiteboards: SessionWhiteboard[]
  openWhiteboard(w: SessionWhiteboard): void
  /** The owner engine is a sandbox VM with a desktop this window can show. */
  vm: { name: string; open(): void } | null
  codemap: boolean
}

/** Everything the tray shows for `session`, behind the same gates the header used. */
export function useTray(session: string): Tray {
  const discovery = useConnection((s) => s.discovery)
  const host = useTileHost()
  const mobile = useIsMobile()
  const navigate = useNavigate()
  const api = useApi()
  const browser = useSessionField(session, 'browser')
  const browserBusy = useBrowserBusy(session)
  // The agent's headless browser (where this engine serves it) and the chat's `show_web` pages, closed ones included;
  // gated on the agent's browser alone, since a page needs no `browser_*`.
  const browsers = useBrowserTargets(session, !!browser && supports(discovery, browserRequirements))
  const artifactMap = useSessionField(session, 'artifacts')
  const artifactsOk = supports(discovery, artifactRequirements)
  const artifacts = useMemo(() => (artifactsOk ? [...(artifactMap?.values() ?? [])].sort((x, y) => y.created_at - x.created_at) : []), [artifactMap, artifactsOk])
  const terminals = useHasTerminals(session) && supports(discovery, terminalRequirements)
  const codemap = useCodemapAvailable(session) && supports(discovery, codemapRequirements)
  const whiteboards = useSessionWhiteboards(session)
  const openWb = useOpenWhiteboard(session)
  const openWhiteboard = (w: SessionWhiteboard) => openWb({ doc: w.doc, title: w.title })
  // 79 §6.5: the owner engine is a sandbox VM (`SessionSummary.owner` ↔ `SandboxInfo.engine`) → its desktop is one click away.
  const canSessions = useQuerySupport(['list_sessions'])
  const sessions = useQuery({ queryKey: queryKeys.sessions, queryFn: () => api.list_sessions(), enabled: canSessions })
  const owner = sessions.data?.find((s) => s.id === session)?.owner
  const sandboxes = useSandboxes()
  const sandbox = owner && supports(discovery, vmRequirements) ? sandboxes.data?.find((s) => s.engine === owner && s.vm?.desktop?.enabled) : undefined
  const vm =
    sandbox && (host !== null || canPopout())
      ? {
          name: sandbox.name,
          // A desktop already in its own window is focused there, never opened twice (one RFB session per VM).
          open: () => {
            const target = { kind: 'vm', name: sandbox.name } as const
            void (async () => {
              if (host === null || (await popoutOpen(target))) await openPopout(target)
              else openVmTile(host, sandbox.name)
            })().catch((e: unknown) => console.warn('open_window', e))
          },
        }
      : null
  const openBrowser = (t: BrowserTarget) =>
    openTarget(t, {
      session,
      host,
      full: mobile && nativeWebViews(),
      openAgent: () => (host === null ? useSessions.getState().setBrowserLive(session, true) : void openBrowserTile(host)),
      route: (w) => void navigate({ to: '/web/$id', params: { id: w.id }, search: { url: w.url, title: w.title } }),
    })
  return { session, terminals, browsers, browserBusy, openBrowser, artifacts, whiteboards, openWhiteboard, vm, codemap }
}

/** The tray has something besides the background work `BackgroundArea` counts itself. */
export const trayHasItems = (t: Tray): boolean => t.terminals || !!t.browsers.length || !!t.whiteboards.length || !!t.artifacts.length || !!t.vm || t.codemap

/** The artifacts, newest first: one opens on a click, several list first (the header's `▣ N` dropdown). */
export function ArtifactsButton({ session, artifacts, className, children }: { session: string; artifacts: Artifact[]; className?: string; children: ReactNode }) {
  const only = artifacts.length === 1 ? artifacts[0] : undefined
  if (only) {
    return (
      <button type="button" className={className} title={`Open ${only.title}`} onClick={() => showArtifact(session, only.call_id)}>
        {children}
      </button>
    )
  }
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button type="button" className={className} title={`Artifacts (${artifacts.length})`}>
          {children}
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-72">
        {artifacts.map((a) => (
          <DropdownMenuItem key={a.call_id} className="flex-col items-start gap-0" onSelect={() => showArtifact(session, a.call_id)}>
            <span className="w-full truncate">{a.title}</span>
            <span className="w-full truncate text-xs text-fg-tertiary">
              {metaLine(a)}
              {a.version > 1 ? ` · v${a.version}` : ''}
            </span>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/** The whiteboards: one opens on a click, several list first (like the artifacts). */
function WhiteboardsButton({ tray, className, children }: { tray: Tray; className?: string; children: ReactNode }) {
  const only = tray.whiteboards.length === 1 ? tray.whiteboards[0] : undefined
  if (only) {
    return (
      <button type="button" className={className} title={`Open ${only.title}`} onClick={() => tray.openWhiteboard(only)}>
        {children}
      </button>
    )
  }
  return (
    <DropdownMenu>
      <DropdownMenuTrigger asChild>
        <button type="button" className={className} title={`Whiteboards (${tray.whiteboards.length})`}>
          {children}
        </button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="start" className="w-72">
        {tray.whiteboards.map((w) => (
          <DropdownMenuItem key={w.doc} onSelect={() => tray.openWhiteboard(w)}>
            <span className="w-full truncate">{w.title}</span>
          </DropdownMenuItem>
        ))}
      </DropdownMenuContent>
    </DropdownMenu>
  )
}

/** A tile: ≥ 44 px to tap, its dash, name, one line of detail and the count top-right. */
const TILE =
  'relative flex min-h-11 w-full min-w-0 flex-col items-start gap-0.5 rounded-xl border bg-background px-2.5 py-2 text-left outline-none hover:bg-layer-2 focus-visible:ring-[3px] focus-visible:ring-ring/50 aria-expanded:bg-layer-2'

function TileBody({ color, width, name, detail, count }: { color: string; width?: string; name: string; detail: string; count: number }) {
  return (
    <>
      <span className="flex h-2 items-center" aria-hidden>
        <ActivityDash color={color} width={width} />
      </span>
      <span className="font-medium text-foreground">{name}</span>
      <span className="w-full truncate pr-4 text-fg-tertiary">{detail}</span>
      <span className="absolute top-2 right-2.5 text-fg-tertiary tabular-nums">{count}</span>
    </>
  )
}

/**
 * Mounted only in the opened tray (`useTerminals` polls): one terminal opens on a tap (its pop-out, or in-page on iOS);
 * several open the viewer, which lists them.
 */
function TerminalsTile({ session, running }: { session: string; running: string[] }) {
  const host = useTileHost()
  const listed = useTerminals(session)
  const count = listed.length || running.length
  const one = listed.length === 1 ? { session: listed[0]!.session, job: listed[0]!.id } : !listed.length && running.length === 1 ? { session, job: running[0]! } : null
  const body = <TileBody color={backgroundColors.terminals.bg} name="Terminals" detail={listed.map((t) => t.name ?? t.command).join(' · ') || `${count} running`} count={count} />
  return one ? (
    <TerminalLink session={one.session} job={one.job} label="Open terminal" className={TILE}>{body}</TerminalLink>
  ) : (
    <TerminalButton session={session} className={TILE} onOpen={() => (host === null ? window.dispatchEvent(new CustomEvent('theuth:terminals', { detail: { session } })) : void openTerminalsTile(host))}>{body}</TerminalButton>
  )
}

/**
 * The opened tray's grid, below the to-dos: one tile per kind with something in it. A tile with one item opens it; one
 * with several opens the list the header used to show (terminals: the viewer; subagents and jobs: their card below).
 */
export function TrayTiles({ tray }: { tray: Tray }) {
  const { session } = tray
  const { sessions: store } = useScope()
  const children = useSessionField(session, 'children')
  const agents = useMemo(() => liveRows(children ?? {}), [children])
  const jobs = useSessionField(session, 'jobs')
  const ptys = useSessionField(session, 'ptys')
  const terminals = useSessionField(session, 'terminals')
  const subagentsExpanded = useSessionField(session, 'subagentsExpanded') ?? false
  const jobsExpanded = useSessionField(session, 'jobsExpanded') ?? false
  const codemap = useSessionField(session, 'codemap')
  const running = useMemo(() => [...new Set([...(terminals ?? []), ...(ptys ?? [])])], [terminals, ptys])
  const jobCount = jobs?.size ?? 0
  const tiles: ReactNode[] = []
  if (tray.terminals || running.length) tiles.push(<TerminalsTile key="terminals" session={session} running={running} />)
  if (tray.browsers.length) {
    tiles.push(
      <BrowserButton key="browser" targets={tray.browsers} busy={tray.browserBusy} onOpen={tray.openBrowser} className={TILE}>
        <TileBody color={backgroundColors.browser.bg} name="Browser" detail={tray.browsers.map((t) => (t.kind === 'web' ? hostOf(t.url) || t.title : targetLabel(t))).join(' · ')} count={tray.browsers.length} />
      </BrowserButton>,
    )
  }
  if (tray.whiteboards.length) {
    tiles.push(
      <WhiteboardsButton key="whiteboards" tray={tray} className={TILE}>
        <TileBody color={backgroundColors.whiteboard.bg} name="Whiteboard" detail={tray.whiteboards.map((w) => w.title).join(' · ')} count={tray.whiteboards.length} />
      </WhiteboardsButton>,
    )
  }
  if (tray.artifacts.length) {
    tiles.push(
      <ArtifactsButton key="artifacts" session={session} artifacts={tray.artifacts} className={TILE}>
        <TileBody color={backgroundColors.artifacts.bg} width={backgroundColors.artifacts.dash} name="Artifacts" detail={tray.artifacts.map((a) => a.title).join(' · ')} count={tray.artifacts.length} />
      </ArtifactsButton>,
    )
  }
  if (agents.length) {
    const body = <TileBody color={backgroundColors.subagents.bg} name="Subagents" detail={agents.map((a) => a.name).join(' · ')} count={agents.length} />
    tiles.push(
      agents.length === 1 ? (
        <SubagentLink key="subagents" row={agents[0]!} className={TILE}>{body}</SubagentLink>
      ) : (
        <button key="subagents" type="button" className={TILE} aria-expanded={subagentsExpanded} onClick={() => store.getState().setBackgroundExpanded(session, 'subagents', !subagentsExpanded)}>{body}</button>
      ),
    )
  }
  if (jobCount) {
    tiles.push(
      <button key="jobs" type="button" className={TILE} aria-expanded={jobsExpanded} onClick={() => store.getState().setBackgroundExpanded(session, 'jobs', !jobsExpanded)}>
        <TileBody color={backgroundColors.jobs.bg} name="Jobs" detail={`${jobCount} running`} count={jobCount} />
      </button>,
    )
  }
  if (tray.vm) {
    tiles.push(
      <button key="desktop" type="button" className={TILE} aria-label={`Desktop of ${tray.vm.name}`} onClick={tray.vm.open}>
        <TileBody color={backgroundColors.desktop.bg} name="Desktop" detail={tray.vm.name} count={1} />
      </button>,
    )
  }
  if (tray.codemap) {
    const roots = codemap?.roots ?? []
    tiles.push(
      <CodeMapButton key="codemap" session={session} className={TILE}>
        <TileBody color={backgroundColors.codemap.bg} name="Code map" detail={roots.map((r) => r.split('/').filter(Boolean).at(-1) ?? r).join(' · ')} count={roots.length} />
      </CodeMapButton>,
    )
  }
  if (!tiles.length) return null
  return <div data-tray-tiles="" className="grid shrink-0 grid-cols-2 gap-1.5 text-xs">{tiles}</div>
}
