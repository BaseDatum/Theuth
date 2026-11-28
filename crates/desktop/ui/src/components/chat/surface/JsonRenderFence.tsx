import { JSONUIProvider, Renderer, type Spec } from '@json-render/react'
import { Component, createContext, type ReactNode, useContext, useEffect, useMemo, useRef, useState } from 'react'
import { cn } from '@/lib/utils'
import { motionAllowed } from '@/lib/animations'
import { claimSurfaceEntrance } from '../entrance'
import type { CustomRendererProps } from 'streamdown'
import { Collapsible, CollapsibleContent, CollapsibleTrigger } from '@/components/ui/collapsible'
import { useSnapshot } from '@/lib/snapshot-mode'
import { DisclosureScope, FenceScope, useScopedDisclosure } from '../disclosure'
import { type ParseResult, parseSpec } from './parse'
import { openSpec } from './streaming'
import { registry } from './registry'
import { unwrapRoot, useBare } from './bare'

/**
 * What a fence renderer knows about its place in the transcript (30 §8), set by `MessageAssistant`: whether the turn
 * that wrote this message still runs (fence-skel D2: that turn, never a later one), whether a later assistant reply
 * follows this message before the next human one, the session an `Open as app` may publish into (`undefined` outside a
 * live transcript: artifact viewer, tool cards), and the assistant message id (135 §2: the first half of a surface's
 * remembered-height key). `readOnly`: an inspection page — no actions that change the transcript (220 Make runnable).
 * The default is the static, settled case.
 */
export type FenceContext = { running: boolean; hasLaterAssistant: boolean; session?: string; entry?: string; readOnly?: boolean }
export const FenceContext = createContext<FenceContext>({ running: false, hasLaterAssistant: false })

/**
 * The closed-fence failure state of every surface fence (mermaid, json-render, openui) — quiet, never a red card, never
 * a placeholder (30 §8, fence-skel D1): while the turn that wrote the fence still runs, or once a later reply of that
 * turn exists, nothing — the engine's correction note tells the model the user was shown nothing, and its fix streams
 * in below. Only a fence the turn ended on says so, in one dim line with the source behind a disclosure. `title` and
 * `error` survive as the tooltip for the curious.
 * Exported as `ErrorCard` too: `Mermaid.tsx` calls it by that name.
 */
export function FenceFallback({ error, source, title }: { error: string; source: string; title?: string }) {
  const { running, hasLaterAssistant } = useContext(FenceContext)
  const [open, setOpen] = useScopedDisclosure('source')
  // 164 D3: a channel picture of a broken fence is no picture; the snapshot answers `failed`.
  const snap = useSnapshot()
  useEffect(() => {
    snap?.fail(title ? `${title}: ${error}` : error)
  }, [snap, title, error])
  if (running || hasLaterAssistant) return null
  return (
    <div className="my-2 text-xs text-fg-tertiary" title={title ? `${title} · ${error}` : error}>
      ◌ diagram omitted ·{' '}
      <Collapsible open={open} onOpenChange={setOpen} className="inline">
        <CollapsibleTrigger className="inline cursor-pointer">show source</CollapsibleTrigger>
        <CollapsibleContent>
          <pre className="mt-2 max-h-56 overflow-auto rounded-lg bg-layer-2 p-3 font-mono text-xs leading-5 wrap-anywhere whitespace-pre-wrap">{source}</pre>
        </CollapsibleContent>
      </Collapsible>
    </div>
  )
}
export { FenceFallback as ErrorCard }

/**
 * Catches a throw inside a catalog component. Closed fence: `fallback(message)` replaces the surface. Open fence
 * (fence-skel D4): the children that last rendered stay up, and the next delta (a new `retry`) tries the live ones again.
 */
class Boundary extends Component<
  { open: boolean; retry: unknown; fallback: (message: string) => ReactNode; children: ReactNode },
  { error: string | null; retry: unknown }
> {
  state = { error: null as string | null, retry: this.props.retry }
  /** The children of the last commit without an error. */
  good: ReactNode = null
  static getDerivedStateFromError(e: unknown) {
    return { error: e instanceof Error ? e.message : String(e) }
  }
  static getDerivedStateFromProps(props: { retry: unknown }, state: { retry: unknown }) {
    return Object.is(props.retry, state.retry) ? null : { error: null, retry: props.retry }
  }
  componentDidMount() {
    this.componentDidUpdate()
  }
  componentDidUpdate() {
    if (this.state.error === null) this.good = this.props.children
  }
  render() {
    if (this.state.error === null) return this.props.children
    return this.props.open ? this.good : this.props.fallback(this.state.error)
  }
}

function Surface({ spec, loading, title }: { spec: Spec; loading: boolean; title?: string }) {
  const bare = useBare()
  const ctx = useContext(FenceContext)
  const snap = useSnapshot()
  const enterRef = useRef<boolean | null>(null)
  if (enterRef.current === null) {
    enterRef.current =
      motionAllowed() && !bare && !snap && !ctx.readOnly && ctx.running
        ? claimSurfaceEntrance(`jr:${ctx.session ?? ''}:${ctx.entry ?? ''}:${spec.root}`)
        : false
  }
  const shown = useMemo(() => (bare ? unwrapRoot(spec) : spec), [bare, spec])
  return (
    <div className={cn(bare ? 'flex h-full min-h-0 flex-col overflow-auto text-sm' : 'my-4 text-sm', enterRef.current && 'paper-surface-enter')} data-streamdown="json-render" data-bare={bare || undefined} title={title}>
      <JSONUIProvider registry={registry}>
        <Renderer spec={shown} registry={registry} loading={loading} />
      </JSONUIProvider>
    </div>
  )
}

export type JsonRenderFenceProps = CustomRendererProps & {
  /** Optional stable disclosure scope (e.g. Deck section id) so live text updates do not collapse disclosures. */
  disclosureScope?: string
}

/**
 * Streamdown custom renderer for ```` ```json-render ```` fences (design 30 §4 R3/R4, §8).
 * Open fence: repair + partial spec, progressive render; a prefix that parses to nothing keeps the last spec that did
 * (fence-skel D4), so a surface that appeared never blinks out before the fence closes. Closed fence: strict parse
 * (repair on failure, flagged in the tooltip), the quiet `FenceFallback` on failure. Disclosures inside are scoped by
 * the fence (`FenceScope`, 171F B13) or an explicit `disclosureScope` when hosted in a live Deck section.
 */
export function JsonRenderFence({ code, isIncomplete, disclosureScope }: JsonRenderFenceProps) {
  const [kept, setKept] = useState<Spec | null>(null)
  // `kept` is read, not a dependency: a newly kept spec must not re-parse the same prefix.
  const parsed = useMemo<ParseResult | { spec: Spec | null }>(
    () => (isIncomplete ? { spec: openSpec(code, kept) } : parseSpec(code)),
    [code, isIncomplete],
  )
  if (isIncomplete && 'spec' in parsed && parsed.spec !== kept) setKept(parsed.spec)
  const wrapScope = (children: ReactNode) =>
    disclosureScope ? <DisclosureScope value={disclosureScope}>{children}</DisclosureScope> : <FenceScope code={code}>{children}</FenceScope>
  if ('error' in parsed)
    return wrapScope(<FenceFallback error={parsed.error} source={code} title="Interactive view unavailable" />)
  if (!parsed.spec) return null
  return wrapScope(
    // Mid-stream a component throw keeps the last surface that rendered and retries on the next spec; the boundary
    // remounts once, when the fence closes. Not keyed by `code`: a new key per delta would remount the surface (and
    // whatever the reader opened in it) each time.
    <Boundary key={isIncomplete ? 'open' : 'closed'} open={isIncomplete} retry={parsed.spec} fallback={(m) => <FenceFallback error={m} source={code} title="Interactive view unavailable" />}>
      <Surface spec={parsed.spec} loading={isIncomplete} title={'repaired' in parsed && parsed.repaired ? 'JSON repaired' : undefined} />
    </Boundary>,
  )
}
