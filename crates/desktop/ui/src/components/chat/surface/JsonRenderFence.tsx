import { JSONUIProvider, Renderer, type Spec } from '@json-render/react'
import { Component, type ReactNode, useMemo } from 'react'
import type { CustomRendererProps } from 'streamdown'
import { type ParseResult, parsePartial, parseSpec } from './parse'
import { registry } from './registry'

/** jarvis's `SurfaceError`, in the desktop's danger-card styling (ApprovalCard/AskCard shape). */
export function ErrorCard({ error, source, title = 'Interactive view unavailable' }: { error: string; source: string; title?: string }) {
  return (
    <div role="alert" className="my-4 rounded-xl border border-danger-border bg-danger-bg px-4 py-3 text-sm">
      <div className="font-medium text-danger">
        {title} · {error}
      </div>
      <details className="mt-1">
        <summary className="cursor-pointer text-fg-secondary">source</summary>
        <pre className="mt-2 max-h-56 overflow-auto rounded-lg bg-layer-2 p-3 font-mono text-xs leading-5 wrap-anywhere whitespace-pre-wrap">{source}</pre>
      </details>
    </div>
  )
}

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
 * (repair on failure, flagged in the tooltip), located error card on failure.
 */
export function JsonRenderFence({ code, isIncomplete }: CustomRendererProps) {
  const parsed = useMemo<ParseResult | { spec: Spec | null }>(
    () => (isIncomplete ? { spec: parsePartial(code) } : parseSpec(code)),
    [code, isIncomplete],
  )
  if ('error' in parsed) return <ErrorCard error={parsed.error} source={code} />
  if (!parsed.spec) return null
  return (
    // Mid-stream a component throw shows nothing for that frame; the boundary remounts when the fence closes.
    <Boundary key={isIncomplete ? 'open' : code} render={(m) => (isIncomplete ? null : <ErrorCard error={m} source={code} />)}>
      <Surface spec={parsed.spec} loading={isIncomplete} title={'repaired' in parsed && parsed.repaired ? 'JSON repaired' : undefined} />
    </Boundary>
  )
}
