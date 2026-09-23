/**
 * Streamdown custom renderer for ```` ```openui ```` fences (30 §8, 46 §8): the apps renderer inline in a reply.
 * Open fence: progressive render (`isStreaming`). Closed fence: the static program — `$state`, `@Set`/`@Reset`/`@OpenUrl`
 * and inputs work; `Query`/`Mutation`/`@ToAssistant` are inert and say so. `Open as app` commits the source through
 * `publish_app` (no model turn) and opens the record in the artifact panel.
 */
import '@openuidev/react-ui/layered/styles/index.css'
import { ThemeProvider } from '@openuidev/react-ui'
import { check, library } from '@theuth/apps'
import { AppRenderer, type AppRendererProps } from '@theuth/apps/renderer'
import { useContext, useMemo, useState } from 'react'
import { toast } from 'sonner'
import type { CustomRendererProps } from 'streamdown'
import { useDark } from '@/components/apps/theme'
import { Button } from '@/components/ui/button'
import { useApi } from '@/lib/rpc'
import type { Api } from '@/rpc/api'
import { useSessions } from '@/store/sessions'
import { FenceContext, FenceFallback } from './JsonRenderFence'

export const INERT = 'Open as app to run queries and actions'

/** Every tool call fails the same way: the transcript has no app record to bind to. */
export const staticProvider: AppRendererProps['provider'] = () => Promise.reject(new Error(INERT))

const onAction: NonNullable<AppRendererProps['onAction']> = (ev) => {
  if (ev.type === 'open_url') {
    const url = ev.params.url
    if (typeof url === 'string' && /^https?:\/\//i.test(url)) window.open(url, '_blank', 'noopener')
    else toast.error('Only http(s) links open from an app')
  } else if (ev.type === 'continue_conversation') toast(INERT)
}

/** The artifact name: the first `TextContent("## …")` heading slugged, else `reply-<short id>`. */
export function appName(source: string): string {
  const heading = /TextContent\(\s*"##\s*([^"\n]+?)\s*"/.exec(source)?.[1]
  const slug = heading
    ?.toLowerCase()
    .replace(/[^a-z0-9]+/g, '-')
    .replace(/^-+|-+$/g, '')
    .slice(0, 40)
  return slug || `reply-${crypto.randomUUID().slice(0, 8)}`
}

/** `publish_app`, then the panel on the new record; the doc's artifact enters the store first so the panel has it even before `ArtifactShown`. */
export async function openAsApp(api: Pick<Api, 'publish_app'>, session: string, source: string): Promise<void> {
  try {
    const doc = await api.publish_app(session, appName(source), source)
    // The generated binding types u64s as bigint; on the wire they are JSON numbers, as `Artifact` in `rpc/extra` says.
    const artifact = { ...doc.artifact, bytes: Number(doc.artifact.bytes), created_at: Number(doc.artifact.created_at) }
    const st = useSessions.getState()
    st.applyEvents([{ type: 'ArtifactShown', session, call_id: artifact.call_id, artifact }])
    st.setArtifactPanel(session, artifact.call_id)
    toast.success('Published as app')
  } catch (e) {
    toast.error(e instanceof Error ? e.message : String(e))
  }
}

export function OpenUIFence({ code, isIncomplete }: CustomRendererProps) {
  const { session } = useContext(FenceContext)
  const api = useApi()
  const dark = useDark()
  const [busy, setBusy] = useState(false)
  // The closed program's first failure; tool availability is the engine's business (the transcript has no snapshot).
  const error = useMemo(
    () => (isIncomplete ? undefined : check(code, []).diagnostics.find((d) => d.code !== 'tool-unknown')?.message),
    [code, isIncomplete],
  )
  if (error !== undefined) return <FenceFallback error={error} source={code} title="App unavailable" />
  const publish = async () => {
    setBusy(true)
    try {
      await openAsApp(api, session ?? '', code)
    } finally {
      setBusy(false)
    }
  }
  return (
    <div className="my-4 rounded-xl border bg-layer-1" data-streamdown="openui">
      <div className="flex h-8 items-center justify-between pl-3 pr-1 font-mono text-xs text-fg-tertiary">
        openui
        {session !== undefined && (
          <Button variant="ghost" size="sm" disabled={isIncomplete || busy} onClick={() => void publish()}>
            Open as app
          </Button>
        )}
      </div>
      <div className="px-3 pb-3 text-sm [&_a]:underline">
        <ThemeProvider mode={dark ? 'dark' : 'light'}>
          <AppRenderer response={code} library={library} isStreaming={isIncomplete} generation={0} provider={staticProvider} paused={false} onAction={onAction} />
        </ThemeProvider>
      </div>
    </div>
  )
}
