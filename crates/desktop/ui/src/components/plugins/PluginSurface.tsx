// 216 §9.4, contract §12.5: one plugin placement's content, whatever hosts it (popover, sheet, tile, command card).
// The attribution strip sits above and outside the content node; the content switches on `format`: OpenUI through
// `AppRenderer` and the plugin provider, json-render through the chat's json-render renderer, markdown through the
// chat's markdown renderer with raw HTML skipped. `loading` and `error` are host-drawn frames.
import { library } from '@theuth/apps'
import { AppRenderer, type AppRendererProps } from '@theuth/apps/renderer'
import { useNavigate } from '@tanstack/react-router'
import { LoaderCircle, X } from 'lucide-react'
import { type ReactNode, useEffect, useMemo, useRef } from 'react'
import { Streamdown } from 'streamdown'
import { copyToClipboard } from '@/components/apps/openui-actions'
import { OpenUITheme } from '@/components/apps/openui-theme'
import { useDark } from '@/components/apps/theme'
import { markdownComponents, rehypePlugins } from '@/components/chat/MessageAssistant'
import { FRESH } from '@/components/chat/slash'
import { JsonRenderFence } from '@/components/chat/surface/JsonRenderFence'
import { EMPTY, openDraftSession, useDrafts } from '@/lib/drafts'
import { notify as toast } from '@/lib/notify'
import { type CommandCard, setCommandCard, usePluginInvalidated, usePlugins } from '@/lib/plugins'
import { cn } from '@/lib/utils'
import type { SurfaceContent } from '@/rpc/bindings/method.plugins.output'
import { useConnection } from '@/store/connection'
import { useSessions } from '@/store/sessions'
import { Attribution } from './Attribution'
import { pluginProvider } from './provider'

export type PluginSurfaceProps = {
  plugin: string
  placement: string
  session: string | null
  /** `undefined` while the first fetch runs (drawn as loading). */
  content: SurfaceContent | undefined
  /** The placement's icon for the strip. */
  icon?: string
  /** Merged onto the content node (padding, scrolling). */
  className?: string
  /** Host controls at the strip's right end (a card's close). */
  actions?: ReactNode
}

export function PluginSurface({ plugin, placement, session, content, icon, className, actions }: PluginSurfaceProps) {
  return (
    <div data-plugin-surface className="flex min-h-0 min-w-0 flex-1 flex-col">
      <div className="flex shrink-0 items-center gap-2 border-b border-border-subtle px-3 py-1.5">
        <Attribution plugin={plugin} icon={icon} className="min-w-0 flex-1" />
        {actions}
      </div>
      <div data-plugin-content className={cn('min-h-0 flex-1 overflow-auto px-3 py-2 text-sm', className)}>
        <Body plugin={plugin} placement={placement} session={session} content={content} />
      </div>
    </div>
  )
}

/** A command reply's card (216 §9.1): what `plugin_command` answered as `ui`, ephemeral, above the composer of
 * `session` that ran it until closed. */
export function PluginCommandCard({ session }: { session: string | null }) {
  const card = usePlugins((s) => s.card)
  if (!card || card.session !== session) return null
  return <CardBody card={card} />
}

function CardBody({ card }: { card: CommandCard }) {
  const content = useMemo<SurfaceContent>(
    () => ({ plugin: card.plugin, placement: card.slash, revision: 0, state: 'ready', format: card.ui.format, source: card.ui.source, text: card.ui.text, bindings: [], error: null }),
    [card],
  )
  return (
    <div data-plugin-card className="mb-2 flex max-h-72 flex-col overflow-hidden rounded-xl border border-border-subtle bg-elevated shadow-sm">
      <PluginSurface
        plugin={card.plugin}
        placement={card.slash}
        session={card.session}
        content={content}
        actions={
          <button type="button" aria-label="Close" className="shrink-0 rounded-sm text-fg-tertiary hover:text-foreground" onClick={() => setCommandCard(null)}>
            <X className="size-3.5" />
          </button>
        }
      />
    </div>
  )
}

function Body({ plugin, placement, session, content }: Omit<PluginSurfaceProps, 'icon' | 'className'>) {
  if (!content || content.state === 'loading')
    return (
      <div role="status" className="flex items-center gap-2 py-2 text-xs text-fg-tertiary">
        <LoaderCircle className="size-3.5 animate-spin" aria-hidden />
        Loading…
      </div>
    )
  if (content.state === 'error')
    return (
      <p role="alert" className="py-2 text-[13px] break-words text-destructive">
        {content.error ?? 'plugin did not render'}
      </p>
    )
  const source = content.source
  if (source === null || content.format === null) return <Fallback text={content.text} />
  switch (content.format) {
    case 'openui':
      return <OpenUI plugin={plugin} placement={placement} session={session} source={source} content={content} />
    case 'json-render':
      return <JsonRenderFence code={source} language="json-render" isIncomplete={false} />
    case 'markdown':
      return (
        <Streamdown mode="static" skipHtml rehypePlugins={rehypePlugins} components={markdownComponents} className="[&_a]:underline [&_a]:underline-offset-2 [&_p]:my-1">
          {source}
        </Streamdown>
      )
  }
}

const Fallback = ({ text }: { text: string | null }) => <p className="py-2 whitespace-pre-wrap text-fg-secondary">{text ?? 'Nothing to show.'}</p>

function OpenUI({ plugin, placement, session, source, content }: { plugin: string; placement: string; session: string | null; source: string; content: SurfaceContent }) {
  const api = useConnection((s) => s.api)
  const dark = useDark()
  const navigate = useNavigate()
  const provider = useMemo(
    () => (api ? pluginProvider(api, { plugin, placement, session, bindings: content.bindings }) : () => Promise.reject(new Error('not connected'))),
    [api, plugin, placement, session, content.bindings],
  )
  // 216 §6.3: a new revision with the same source keeps the program (and its state) mounted; only a new source reloads.
  const gen = useRef({ source, generation: 0 })
  if (gen.current.source !== source) gen.current = { source, generation: gen.current.generation + 1 }
  // `PluginInvalidate`: re-run the queries without replacing the source.
  const runtime = useRef<NonNullable<AppRendererProps['runtimeRef']>['current']>(null)
  const invalidated = usePluginInvalidated(plugin, placement)
  const seen = useRef(invalidated)
  useEffect(() => {
    if (seen.current === invalidated) return
    seen.current = invalidated
    runtime.current?.refresh()
  }, [invalidated])
  const onAction: NonNullable<AppRendererProps['onAction']> = (ev) => {
    if (ev.type === 'open_url') {
      const url = ev.params.url
      if (typeof url === 'string' && /^https?:\/\//i.test(url)) window.open(url, '_blank', 'noopener')
      else toast.error('Only http(s) links open from a plugin')
    } else if (ev.type === 'CopyToClipboard') copyToClipboard(ev.params)
    else if (ev.type === 'continue_conversation' && api) {
      // 216 §9.4: `@ToAssistant` drafts a new chat with the text; sending stays the owner's press.
      const context = typeof ev.params.context === 'string' ? ev.params.context : ''
      const text = [ev.humanFriendlyMessage, context].filter(Boolean).join('\n\n')
      void openDraftSession(api).then(
        ({ id }) => {
          useSessions.getState().loadHistory(id, FRESH, 'replace')
          useDrafts.getState().set(id, { text, pastes: EMPTY.pastes })
          return navigate({ to: '/c/$session', params: { session: id } })
        },
        (e: unknown) => toast.error(e instanceof Error ? e.message : String(e)),
      )
    }
  }
  return (
    <div className="[&_a]:underline" data-streamdown="openui">
      <OpenUITheme mode={dark ? 'dark' : 'light'}>
        <AppRenderer
          response={source}
          library={library}
          isStreaming={false}
          generation={gen.current.generation}
          provider={provider}
          paused={api === null}
          onAction={onAction}
          runtimeRef={runtime}
        />
      </OpenUITheme>
    </div>
  )
}
