import { AppRenderer, type AppRendererProps } from '@theuth/apps/renderer'
import { Component, type ReactNode, useCallback, useEffect, useMemo, useRef } from 'react'
import { OpenUITheme } from '@/components/apps/openui-theme'
import { useDark } from '@/components/apps/theme'
import type { InvokeActionOptions } from '@/components/chat/surface/deck-context'
import { deckOpenUILibrary, handleDeckOpenUIAction } from './deck-openui-library'
import { analyzeDeckOpenUISource, createDeckOpenUIProvider } from './deck-openui-provider'
import type { DeckRuntimeStatus, InteractResponse, ScriptRevision } from './types'

class OpenUIRenderBoundary extends Component<
  { retryKey: string; children: ReactNode },
  { error: string | null; retryKey: string }
> {
  state = { error: null as string | null, retryKey: this.props.retryKey }

  static getDerivedStateFromError(e: unknown) {
    return { error: e instanceof Error ? e.message : String(e) }
  }

  static getDerivedStateFromProps(
    props: { retryKey: string },
    state: { retryKey: string },
  ) {
    return props.retryKey === state.retryKey
      ? null
      : { error: null, retryKey: props.retryKey }
  }

  render() {
    if (this.state.error !== null) {
      return (
        <div
          role="alert"
          data-deck-openui-runtime-error
          className="rounded-md border border-danger/30 bg-danger/10 px-2.5 py-2 text-xs text-danger"
        >
          OpenUI render error: {this.state.error}
        </div>
      )
    }
    return this.props.children
  }
}

export type DeckOpenUISectionProps = {
  deckId: string
  sectionId: string
  source: string
  dataKey?: string | null
  /** Section-scoped (and data_key-sliced) live/snapshot payload. */
  data: unknown
  /** Monotonic version counter bumped on live frames, snapshots, and runtime updates. */
  dataVersion?: number
  runtime: DeckRuntimeStatus | null
  activeScript: ScriptRevision | null
  pinned?: boolean
  interactive?: boolean
  invokeAction: (
    action: string,
    args: Record<string, unknown>,
    opts?: InvokeActionOptions,
  ) => Promise<InteractResponse | null>
}

/**
 * First-class restricted OpenUI section renderer for Decks.
 * - Fails closed via static `check(source, tools)` against ONLY `deck_data`, `deck_status`,
 *   and declared actions of the approved `activeScript` (never session/MCP tools).
 * - Keeps `generation` stable across live data frames — only bumps on `(source, dataKey, activeScript.rev)`.
 * - On live `dataVersion` / `data` / status changes, calls `runtime.refresh(localQueryIds)` ONLY for
 *   local `deck_data` / `deck_status` statements, preserving local `$state` and preventing remote
 *   Monty query spam on high-frequency frames.
 * - Renders inside `OpenUITheme` + `deckOpenUILibrary` (no password inputs, no raw HTML, external
 *   images blocked, `@ToAssistant` inert, mutations require `interactive && !pinned`).
 */
export function DeckOpenUISection({
  deckId,
  sectionId,
  source,
  dataKey = null,
  data,
  dataVersion = 0,
  runtime,
  activeScript,
  pinned = false,
  interactive = true,
  invokeAction,
}: DeckOpenUISectionProps) {
  const dark = useDark()
  const approvedScript = activeScript?.status === 'approved' ? activeScript : null
  const approvedRev = approvedScript?.rev ?? null

  const analysis = useMemo(
    () => analyzeDeckOpenUISource(source, deckId, approvedScript),
    [source, deckId, approvedScript],
  )

  // Stable generation: bump ONLY when section content, data_key, or approved script rev changes.
  const genKey = `${source}\u0000${dataKey ?? ''}\u0000${approvedRev ?? ''}`
  const genRef = useRef({ key: genKey, generation: 0 })
  if (genRef.current.key !== genKey) {
    genRef.current = { key: genKey, generation: genRef.current.generation + 1 }
  }
  const generation = genRef.current.generation

  // Keep refs synchronized so the memoized provider always observes fresh values without remounting.
  const dataRef = useRef(data)
  dataRef.current = data
  const runtimeStatusRef = useRef(runtime)
  runtimeStatusRef.current = runtime
  const activeScriptRef = useRef(approvedScript)
  activeScriptRef.current = approvedScript
  const pinnedRef = useRef(pinned)
  pinnedRef.current = pinned
  const interactiveRef = useRef(interactive)
  interactiveRef.current = interactive
  const invokeActionRef = useRef(invokeAction)
  invokeActionRef.current = invokeAction

  const provider = useMemo(
    () =>
      createDeckOpenUIProvider({
        deckId,
        sectionId,
        bindings: analysis.bindings,
        getSectionData: () => dataRef.current,
        getRuntime: () => runtimeStatusRef.current,
        getActiveScript: () => activeScriptRef.current,
        isPinned: () => pinnedRef.current,
        isInteractive: () => interactiveRef.current,
        invokeAction: (action, args, opts) => invokeActionRef.current(action, args, opts),
      }),
    [deckId, sectionId, analysis.bindings],
  )

  const appRuntimeRef = useRef<NonNullable<AppRendererProps['runtimeRef']>['current']>(null)

  const statusFingerprint = `${runtime?.state ?? ''}:${runtime?.stale ? 1 : 0}:${runtime?.run_epoch ?? 0}:${runtime?.step_count ?? 0}:${runtime?.last_error ?? ''}:${pinned ? 1 : 0}:${interactive ? 1 : 0}`
  const lastRefreshRef = useRef({
    generation,
    dataVersion,
    data,
    statusFingerprint,
  })

  // Refresh ONLY local `deck_data`/`deck_status` queries on live updates, preserving `$state`
  // and avoiding remote Monty queries on high-frequency frames.
  useEffect(() => {
    const prev = lastRefreshRef.current
    if (prev.generation !== generation) {
      lastRefreshRef.current = { generation, dataVersion, data, statusFingerprint }
      return
    }
    const changed =
      prev.dataVersion !== dataVersion ||
      prev.data !== data ||
      prev.statusFingerprint !== statusFingerprint
    if (!changed) return
    lastRefreshRef.current = { generation, dataVersion, data, statusFingerprint }
    if (analysis.ok && analysis.localQueryIds.length > 0) {
      appRuntimeRef.current?.refresh(analysis.localQueryIds)
    }
  }, [generation, dataVersion, data, statusFingerprint, analysis.ok, analysis.localQueryIds])

  const onAction = useCallback<NonNullable<AppRendererProps['onAction']>>((ev) => {
    handleDeckOpenUIAction(ev)
  }, [])

  if (!analysis.ok) {
    return (
      <div
        role="alert"
        data-deck-openui-section={sectionId}
        data-deck-openui-error
        className="space-y-1 rounded-md border border-danger/30 bg-danger/10 px-2.5 py-2 text-xs text-danger"
      >
        <div className="font-medium">OpenUI section validation failed</div>
        <ul className="list-disc space-y-0.5 pl-4 text-[11px]">
          {analysis.diagnostics.slice(0, 5).map((d, i) => (
            <li key={`${d.at}-${d.code}-${i}`}>
              {d.at ? <code className="font-mono">{d.at}: </code> : null}
              {d.message}
            </li>
          ))}
        </ul>
      </div>
    )
  }

  return (
    <div
      data-deck-openui-section={sectionId}
      data-deck-openui-generation={generation}
      data-deck-openui-interactive={interactive || undefined}
      data-deck-openui-pinned={pinned || undefined}
      data-streamdown="openui"
      className="min-w-0 text-sm [&_a]:underline"
    >
      <OpenUIRenderBoundary retryKey={`${sectionId}:${generation}`}>
        <OpenUITheme mode={dark ? 'dark' : 'light'}>
          <AppRenderer
            response={analysis.source}
            library={deckOpenUILibrary}
            isStreaming={false}
            generation={generation}
            provider={provider}
            onAction={onAction}
            runtimeRef={appRuntimeRef}
          />
        </OpenUITheme>
      </OpenUIRenderBoundary>
    </div>
  )
}
