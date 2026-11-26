import { Component, Fragment, type ErrorInfo, type ReactNode } from 'react'
import { Button } from '@/components/ui/button'

type Props = {
  /** A new value (the next source delta, another record) clears a caught error and renders the children again. */
  resetKey?: unknown
  children: ReactNode
}
type State = { error: string | null; resetKey: unknown; attempt: number }

/**
 * One openui block or app view that throws while rendering (a component bug, a hook-order mismatch, a lazy chunk that
 * failed to load) shows one quiet line with a Retry instead of unmounting the whole transcript or pane. Retry remounts
 * the children from scratch, so whatever state broke them is gone.
 */
export class RenderBoundary extends Component<Props, State> {
  state: State = { error: null, resetKey: this.props.resetKey, attempt: 0 }

  static getDerivedStateFromError(e: unknown): Partial<State> {
    return { error: e instanceof Error ? e.message : String(e) }
  }

  static getDerivedStateFromProps(props: Props, state: State): Partial<State> | null {
    return Object.is(props.resetKey, state.resetKey) ? null : { error: null, resetKey: props.resetKey }
  }

  componentDidCatch(error: unknown, info: ErrorInfo) {
    console.error('openui block failed to render', error, info.componentStack)
  }

  retry = () => this.setState((s) => ({ error: null, attempt: s.attempt + 1 }))

  render() {
    if (this.state.error === null) return <Fragment key={this.state.attempt}>{this.props.children}</Fragment>
    return (
      <div className="my-2 flex items-center gap-2 text-xs text-fg-tertiary" data-render-error="">
        <span className="min-w-0 flex-1 wrap-anywhere">This block failed to render — {this.state.error}</span>
        <Button variant="ghost" size="sm" onClick={this.retry}>
          Retry
        </Button>
      </div>
    )
  }
}
