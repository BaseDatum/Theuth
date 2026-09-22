import { type ReactNode, useEffect, useState } from 'react'
import { toast } from 'sonner'
import { Editor as BlockEditor } from '@/components/editor/Editor'
import { ParticipantStrip } from '@/components/share/ParticipantStrip'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Toaster } from '@/components/ui/sonner'
import { type DocStatus, type GuestUser, HarnessProvider } from '@/lib/doc/provider'
import { CLOSE_REFUSED } from '@/link/frames'
import { type ShareTarget, joinProof, parseShareLink } from '@/link/guest'
import type { Harness } from '@/link/harnesses'
import { idbGet, idbPut } from '@/link/idb'
import { getIdentity } from '@/link/keys'
import { LinkTransport } from '@/link/transport'
import { makeApi } from '@/rpc/api'
import type { DocEvent } from '@/rpc/bindings/DocEvent'
import { RpcClient } from '@/rpc/client'

/** IndexedDB row (`link`/`identity`) holding the name this browser joins with; asked once (59 §8.6). */
const NAME_KEY = 'guest-name'

type GuestState =
  | { kind: 'connecting' }
  | { kind: 'open'; provider: HarnessProvider; user: GuestUser; owner: string }
  /** Terminal: the connection is closed and the page says why. */
  | { kind: 'gone'; message: string }

/** What a `doc_join` / `doc_open` refusal means to the reader (59 §8.6). */
function goneMessage(code: string): string {
  if (code.includes('link_revoked') || code.includes('link_expired') || code.includes('link_unknown') || code.includes('bad_proof')) return 'This link no longer works.'
  if (code.includes('members_full')) return 'This document is full.'
  if (code.includes('sharing')) return 'Sharing is turned off for this document.'
  if (code.includes('not_member')) return 'Your access was removed.'
  return 'Could not open this document.'
}

/**
 * The `/d` guest container (59 §8.6): one document, over the same Noise channel a paired phone uses.
 * Parse the fragment, ask a name once, connect as a guest (31 §4.3 mode 2), `doc_join` with the
 * connection-bound proof, then the ordinary `HarnessProvider` on the document.
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
    const harness: Harness = { id: target.harnessId, name: '', relayHost: target.relayHost, secure: target.secure, pairedAt: Date.now(), lastSeen: 0, order: 0 }
    let provider: HarnessProvider | null = null
    let ended = false
    const end = (message: string) => {
      if (ended) return
      ended = true
      provider?.destroy()
      provider = null
      client.close()
      setState({ kind: 'gone', message })
    }
    const transport = new LinkTransport(harness, {
      onClose: (reason) => {
        // The harness refused us after the handshake (not admitted in time, or three refusals): stop retrying.
        if (reason === CLOSE_REFUSED) end(provider ? 'Your access was removed.' : 'This link no longer works.')
      },
    })
    const client = new RpcClient(transport)
    const api = makeApi(client)
    const open = (user: GuestUser) => {
      provider?.destroy()
      const p = new HarnessProvider(client, api, target.doc, target.harnessId, user)
      provider = p
      p.onStatus((s) => {
        if (s.kind !== 'closed' || p !== provider) return
        if (s.reason === 'demoted') {
          toast('You can now only view this document')
          open({ ...user, role: 'viewer' })
        } else if (s.reason === 'revoked') end('Your access was removed.')
        else if (s.reason === 'deleted') end('This document was deleted.')
      })
      setState({ kind: 'open', provider: p, user, owner: transport.hello?.name ?? '' })
    }
    let joined = false
    const off = client.onState((s) => {
      if (s !== 'open' || joined || ended) return
      joined = true
      void (async () => {
        try {
          const h = transport.handshakeHash
          if (!h) throw new Error('no handshake')
          const me = await getIdentity()
          const j = await api.doc_join(target.doc, await joinProof(target.secret, h, me.pub), name)
          if (!ended) open({ name: j.name, color: j.color, role: j.role })
        } catch (e) {
          end(goneMessage(e instanceof Error ? e.message : String(e)))
        }
      })()
    })
    void client.connect().catch(() => undefined)
    return () => {
      off()
      ended = true
      provider?.destroy()
      client.close()
    }
  }, [target, name])

  if (state.kind === 'gone') return <Screen title={state.message} />
  if (state.kind === 'connecting') return <Screen title="Connecting…" hint="Waiting for the owner's Mac through the relay." />
  return <GuestDocument key={state.provider.doc.clientID} provider={state.provider} user={state.user} owner={state.owner} />
}

function GuestDocument({ provider, user, owner }: { provider: HarnessProvider; user: GuestUser; owner: string }) {
  const [status, setStatus] = useState<DocStatus>(provider.status)
  const [title, setTitle] = useState(provider.meta?.title ?? '')
  useEffect(() => {
    setStatus(provider.status)
    setTitle(provider.meta?.title ?? '')
    const offStatus = provider.onStatus((s) => {
      setStatus(s)
      setTitle(provider.meta?.title ?? '')
    })
    const offEvent = provider.onEvent((e: DocEvent) => {
      if (e.type === 'Members') setTitle(provider.meta?.title ?? '')
    })
    return () => {
      offStatus()
      offEvent()
    }
  }, [provider])
  return (
    <main className="flex h-dvh flex-col" data-guest-role={user.role}>
      <header className="flex h-[52px] shrink-0 items-center gap-3 border-b border-border-subtle px-4">
        <h1 className="min-w-0 flex-1 truncate text-sm font-medium">{title || 'Untitled'}</h1>
        <ParticipantStrip provider={provider} />
      </header>
      <BlockEditor provider={provider} status={status} />
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
