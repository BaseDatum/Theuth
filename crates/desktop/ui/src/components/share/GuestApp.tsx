import { QueryClient, QueryClientProvider } from '@tanstack/react-query'
import { type ReactNode, useEffect, useMemo, useState } from 'react'
import { toast } from 'sonner'
import { Editor as BlockEditor } from '@/components/editor/Editor'
import { NotebookGuest } from '@/components/notebooks/NotebookGuest'
import { ParticipantStrip } from '@/components/share/ParticipantStrip'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Toaster } from '@/components/ui/sonner'
import { type DocStatus, type GuestUser, EngineProvider } from '@/lib/doc/provider'
import { CLOSE_REFUSED } from '@/link/frames'
import { type ShareTarget, dialRotation, joinBusy, joinProof, parseShareLink, serverMoved } from '@/link/guest'
import type { Engine } from '@/link/engines'
import { idbGet, idbPut } from '@/link/idb'
import { getIdentity } from '@/link/keys'
import { LinkTransport } from '@/link/transport'
import { type Api, makeApi } from '@/rpc/api'
import type { DocEvent } from '@/rpc/bindings/DocEvent'
import type { DocKind } from '@/rpc/bindings/DocKind'
import { RpcClient } from '@/rpc/client'

/** IndexedDB row (`link`/`identity`) holding the name this browser joins with; asked once (59 §8.6). */
const NAME_KEY = 'guest-name'

type GuestState =
  | { kind: 'connecting' }
  | { kind: 'open'; provider: EngineProvider; user: GuestUser; owner: string; client: RpcClient; engine: string }
  /** Terminal: the connection is closed and the page says why. */
  | { kind: 'gone'; message: string }

/** What a `doc_join` / `doc_open` refusal means to the reader (59 §8.6). */
function goneMessage(code: string): string {
  if (code.includes('link_revoked') || code.includes('link_expired') || code.includes('link_unknown') || code.includes('bad_proof')) return 'This link no longer works.'
  if (code.includes('members_full')) return 'This document is full.'
  if (code.includes('sharing')) return 'Sharing is turned off for this document.'
  if (code.includes('not_member') || code.includes('member_revoked')) return 'Your access was removed.'
  return 'Could not open this document.'
}

/**
 * The `/d` guest container (59 §8.6): one document, over the same Noise channel a paired phone uses.
 * Parse the fragment, ask a name once, connect as a guest (31 §4.3 mode 2), `doc_join` with the
 * connection-bound proof, then the ordinary `EngineProvider` on the document.
 */
export function GuestApp() {
  const [target] = useState(() => parseShareLink(location.hash))
  const [name, setName] = useState<string | null | undefined>(undefined)
  useEffect(() => {
    idbGet<string>('identity', NAME_KEY)
      .then((n) => setName(n ?? null))
      .catch(() => setName(null))
  }, [])
  if (!target) return <Screen title="This link no longer works." hint="Ask the person who shared the document for a new link." />
  if (name === undefined) return null
  if (name === null) {
    return (
      <NamePrompt
        onSubmit={(n) => {
          void idbPut('identity', NAME_KEY, n).catch(() => undefined)
          setName(n)
        }}
      />
    )
  }
  return (
    <>
      <GuestSession target={target} name={name} />
      <Toaster />
    </>
  )
}

function Screen({ title, hint, children }: { title: string; hint?: string; children?: ReactNode }) {
  return (
    <main className="flex h-dvh flex-col items-center justify-center gap-3 p-6 text-center">
      <h1 className="text-lg font-medium">{title}</h1>
      {hint && <p className="max-w-sm text-sm text-fg-tertiary">{hint}</p>}
      {children}
      <GetTheuth />
    </main>
  )
}

function GetTheuth() {
  return (
    <footer className="mt-6 text-xs text-fg-tertiary">
      Shared with{' '}
      <a className="underline" href="https://theuth.io" target="_blank" rel="noreferrer">
        theuth
      </a>{' '}
      · Get theuth to share your own documents.
    </footer>
  )
}

function NamePrompt({ onSubmit }: { onSubmit: (name: string) => void }) {
  const [draft, setDraft] = useState('')
  const name = draft.trim()
  return (
    <main className="flex h-dvh flex-col items-center justify-center gap-4 p-6">
      <form
        className="flex w-full max-w-xs flex-col gap-3"
        onSubmit={(e) => {
          e.preventDefault()
          if (name) onSubmit(name.slice(0, 64))
        }}
      >
        <h1 className="text-lg font-medium">Someone shared a document with you</h1>
        <p className="text-sm text-fg-tertiary">What should the others call you?</p>
        <Input autoFocus aria-label="Your name" placeholder="Your name" value={draft} onChange={(e) => setDraft(e.target.value)} />
        <Button type="submit" disabled={!name}>
          Continue
        </Button>
      </form>
      <GetTheuth />
    </main>
  )
}

/** Connect, join, open; `demoted` re-opens read-only, `revoked`/`deleted`/a refused link end the session. */
function GuestSession({ target, name }: { target: ShareTarget; name: string }) {
  const [state, setState] = useState<GuestState>({ kind: 'connecting' })
  useEffect(() => {
    // §12.6: the store key is the first id; the dialled id (home, then mesh) changes per connection.
    const engineId = target.engineIds[0] ?? ''
    let provider: EngineProvider | null = null
    let ended = false
    let cur: { client: RpcClient; transport: LinkTransport; api: Api } | null = null
    const end = (message: string) => {
      if (ended) return
      ended = true
      rotation.stop()
      provider?.destroy()
      provider = null
      cur?.client.close()
      setState({ kind: 'gone', message })
    }
    const open = (user: GuestUser) => {
      const c = cur
      if (!c) return
      provider?.destroy()
      const p = new EngineProvider(c.client, c.api, target.doc, engineId, user)
      provider = p
      p.onStatus((s) => {
        if (s.kind !== 'closed' || p !== provider) return
        if (s.reason === 'demoted') {
          toast('You can now only view this document')
          open({ ...user, role: 'viewer' })
        } else if (s.reason === 'revoked') end('Your access was removed.')
        else if (s.reason === 'deleted') end('This document was deleted.')
        else if (serverMoved(s.reason)) retryJoin(c)
        else if (s.reason.includes('not_member')) {
          // A server move forgets guests: join again once, then it is a real removal (§12.6).
          if (rejoined) end('Your access was removed.')
          else {
            rejoined = true
            void join(c)
          }
        }
      })
      setState({ kind: 'open', provider: p, user, owner: c.transport.hello?.name ?? '', client: c.client, engine: engineId })
    }
    let rejoined = false
    // The server moved under us: join again on this connection after a beat, as after a drop.
    const retryJoin = (c: NonNullable<typeof cur>) => {
      setTimeout(() => !ended && c === cur && void join(c), 1000)
    }
    const join = async (c: NonNullable<typeof cur>) => {
      try {
        const h = c.transport.handshakeHash
        if (!h) throw new Error('no handshake')
        const me = await getIdentity()
        const j = await joinBusy(async () => c.api.doc_join(target.doc, await joinProof(target.secret, h, me.pub), name))
        if (!ended && c === cur) open({ name: j.name, color: j.color, role: j.role })
      } catch (e) {
        const msg = e instanceof Error ? e.message : String(e)
        if (serverMoved(msg)) retryJoin(c)
        else end(goneMessage(msg))
      }
    }
    const rotation = dialRotation(target.engineIds, (id) => {
      const engine: Engine = { id, name: '', relayHost: target.relayHost, secure: target.secure, pairedAt: Date.now(), lastSeen: 0, order: 0, kind: 'engine' }
      const transport = new LinkTransport(engine, {
        onClose: (reason) => {
          // The engine refused us after the handshake (not admitted in time, or three refusals): stop retrying.
          if (reason === CLOSE_REFUSED && cur?.transport === transport) end(provider ? 'Your access was removed.' : 'This link no longer works.')
        },
      })
      const client = new RpcClient(transport)
      const c = { client, transport, api: makeApi(client) }
      cur = c
      rejoined = false
      let joined = false
      client.onState((s) => {
        if (ended || cur !== c) return
        if (s !== 'open') return rotation.lost()
        rotation.open()
        if (joined) return
        joined = true
        void join(c)
      })
      void client.connect().catch(() => undefined)
      return client
    })
    rotation.start()
    return () => {
      ended = true
      rotation.stop()
      provider?.destroy()
      cur?.client.close()
    }
  }, [target, name])

  if (state.kind === 'gone') return <Screen title={state.message} />
  if (state.kind === 'connecting') return <Screen title="Connecting…" hint="Waiting for the owner's Mac through the relay." />
  return <GuestDocument key={state.provider.doc.clientID} provider={state.provider} user={state.user} owner={state.owner} client={state.client} engine={state.engine} />
}

function GuestDocument({ provider, user, owner, client, engine }: { provider: EngineProvider; user: GuestUser; owner: string; client: RpcClient; engine: string }) {
  const [status, setStatus] = useState<DocStatus>(provider.status)
  const [title, setTitle] = useState(provider.meta?.title ?? '')
  // 171 §13: note cells read their replicated latest run (`cell_latest`) through this connection; the guest page has
  // no engine scope, so it brings the query client the cell chrome uses.
  const api = useMemo(() => makeApi(client), [client])
  const [qc] = useState(() => new QueryClient())
  useEffect(() => {
    setStatus(provider.status)
    setTitle(provider.meta?.title ?? '')
    const offStatus = provider.onStatus((s) => {
      setStatus(s)
      setTitle(provider.meta?.title ?? '')
    })
    const offEvent = provider.onEvent((e: DocEvent) => {
      if (e.type === 'Members' || e.type === 'Meta') setTitle(provider.meta?.title ?? '')
    })
    return () => {
      offStatus()
      offEvent()
    }
  }, [provider])
  useEffect(() => {
    document.title = title
  }, [title])
  return (
    <main className="flex h-dvh flex-col" data-guest-role={user.role}>
      <header className="flex h-[52px] shrink-0 items-center gap-3 border-b border-border-subtle px-4">
        <h1 className="min-w-0 flex-1 truncate text-sm font-medium">{title || 'Untitled'}</h1>
        <ParticipantStrip provider={provider} />
      </header>
      {/* 144 §12.5: a shared notebook is its three columns, not a paper (`notebook` joins `DocKind` with the bindings). Until `meta` lands with `doc_open` the kind is unknown, so nothing rather than an editor that flips. */}
      {provider.meta?.kind === ('notebook' as DocKind) ? (
        <NotebookGuest provider={provider} client={client} engine={engine} user={user} role={user.role} />
      ) : !provider.meta && status.kind === 'connecting' ? (
        <div className="flex-1" />
      ) : (
        <QueryClientProvider client={qc}>
          <BlockEditor provider={provider} status={status} guest={api} />
        </QueryClientProvider>
      )}
      <footer className="flex h-8 shrink-0 items-center justify-between gap-3 border-t border-border-subtle px-4 text-xs text-fg-tertiary">
        <span data-testid="guest-you">
          You are {user.role === 'viewer' ? 'viewing' : 'editing'} as <span style={{ color: user.color }}>{user.name}</span>
          {owner ? ` · shared by ${owner}` : ''}
        </span>
        <a className="underline" href="https://theuth.io" target="_blank" rel="noreferrer">
          Get theuth
        </a>
      </footer>
    </main>
  )
}
