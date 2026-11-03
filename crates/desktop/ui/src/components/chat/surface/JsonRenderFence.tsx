import { JSONUIProvider, Renderer, type Spec } from '@json-render/react'
import { Component, createContext, type ReactNode, useContext, useEffect, useMemo } from 'react'
import type { CustomRendererProps } from 'streamdown'
import { useSnapshot } from '@/lib/snapshot-mode'
import { type ParseResult, parsePartial, parseSpec } from './parse'
import { registry } from './registry'

/**
 * What a fence renderer knows about its place in the transcript (30 §8), set by `MessageAssistant`: whether the
 * session's turn still runs, whether a later assistant reply follows this message before the next human one, the
 * session an `Open as app` may publish into (`undefined` outside a live transcript: artifact viewer, tool cards), and
 * the assistant message id (135 §2: the first half of a surface's remembered-height key).
 * The default is the static, settled case.
 */
export type FenceContext = { running: boolean; hasLaterAssistant: boolean; session?: string; entry?: string }
export const FenceContext = createContext<FenceContext>({ running: false, hasLaterAssistant: false })

/**
 * The closed-fence failure state of every surface fence (mermaid, json-render, openui) — quiet, never a red card
 * (30 §8): a later reply in the same turn carries the fix, so the broken fence renders nothing; while the turn still
 * runs (the engine is asking the model to fix it) a skeleton; only a fence the turn ended on says so, in one dim line
 * with the source behind a disclosure. `title` and `error` survive as the tooltip for the curious.
 * Exported as `ErrorCard` too: `Mermaid.tsx` calls it by that name.
 */
export function FenceFallback({ error, source, title }: { error: string; source: string; title?: string }) {
  const { running, hasLaterAssistant } = useContext(FenceContext)
  // 164 D3: a channel picture of a broken fence is no picture; the snapshot answers `failed`.
  const snap = useSnapshot()
  useEffect(() => {
    snap?.fail(title ? `${title}: ${error}` : error)
  }, [snap, title, error])
  if (hasLaterAssistant) return null
  if (running) return <div className="my-4 h-24 animate-pulse rounded-xl bg-layer-2" aria-busy="true" />
  return (
    <div className="my-2 text-xs text-fg-tertiary" title={title ? `${title} · ${error}` : error}>
      ◌ diagram omitted ·{' '}
      <details className="inline">
        <summary className="inline cursor-pointer">show source</summary>
        <pre className="mt-2 max-h-56 overflow-auto rounded-lg bg-layer-2 p-3 font-mono text-xs leading-5 wrap-anywhere whitespace-pre-wrap">{source}</pre>
      </details>
    </div>
  )
}
export { FenceFallback as ErrorCard }

/** Catches a throw inside a catalog component; `render(message)` replaces the surface. */
class Boundary extends Component<{ render: (message: string) => ReactNode; children: ReactNode }, { error: string | null }> {
  state: { error: string | null } = { error: null }
  static getDerivedStateFromError(e: unknown) {
    return { error: e instanceof Error ? e.message : String(e) }
  }
  render() {
    return this.state.error === null ? this.props.children : this.props.render(this.state.error)
  }
}

function Surface({ spec, loading, title }: { spec: Spec; loading: boolean; title?: string }) {
  return (
    <div className="my-4 text-sm" data-streamdown="json-render" title={title}>
      <JSONUIProvider registry={registry}>
        <Renderer spec={spec} registry={registry} loading={loading} />
      </JSONUIProvider>
    </div>
  )
}

/**
 * Streamdown custom renderer for ```` ```json-render ```` fences (design 30 §4 R3/R4, §8).
 * Open fence: repair + partial spec, progressive render, nothing on failure. Closed fence: strict parse
 * (repair on failure, flagged in the tooltip), the quiet `FenceFallback` on failure.
 */
export function JsonRenderFence({ code, isIncomplete }: CustomRendererProps) {
  const parsed = useMemo<ParseResult | { spec: Spec | null }>(
    () => (isIncomplete ? { spec: parsePartial(code) } : parseSpec(code)),
    [code, isIncomplete],
  )
  if ('error' in parsed) return <FenceFallback error={parsed.error} source={code} title="Interactive view unavailable" />
  if (!parsed.spec) return null
  return (
    // Mid-stream a component throw shows nothing for that frame; the boundary remounts when the fence closes.
    <Boundary key={isIncomplete ? 'open' : code} render={(m) => (isIncomplete ? null : <FenceFallback error={m} source={code} title="Interactive view unavailable" />)}>
      <Surface spec={parsed.spec} loading={isIncomplete} title={'repaired' in parsed && parsed.repaired ? 'JSON repaired' : undefined} />
    </Boundary>
  )
}
