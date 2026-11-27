// 224 §16.12: a phone-only cloud node (made by the iPhone's own engine, which is not a mesh member) forms its own mesh;
// this device then pairs with the node's engine over the relay — the same pairing as any remote engine — and switches
// to it. The pairing string comes from the phone's engine once (`cloud_link_pairing`, built from the node's answer on
// the relay and the one-time claim's PSK); after the pairing the phone's engine forgets the PSK (`cloud_link_paired`).
// The master password typed in the wizard goes to the node right after, over that link, with `vault_add_password(null,
// pw)`: the node is the connected engine of that client. It never reaches the phone's engine, the user data or a log.

import { useEffect, useRef, useState } from 'react'
import { vaultApi, isUnsupported } from '@/components/vault/api'
import { deviceName } from '@/lib/platform'
import { useEngines, type Engine } from '@/link/engines'
import { pair } from '@/link/pairing'
import { LinkTransport } from '@/link/transport'
import { RpcClient } from '@/rpc/client'
import type { CloudApi, CloudNode } from './types'

/** What happened to the master password on the node. */
export type VaultOutcome = 'set' | 'none' | 'unsupported' | 'failed'

export type LinkDeps = {
  api: Pick<CloudApi, 'cloud_link_pairing' | 'cloud_link_paired'>
  pair: (key: string, deviceName: string) => Promise<Engine>
  /** A raw client on the paired engine (connected), closed after the vault call. */
  open: (h: Engine) => Promise<{ call<T>(method: string, params: unknown[]): Promise<T>; close(): void }>
  deviceName: string
}

const liveDeps = (api: LinkDeps['api']): LinkDeps => ({
  api,
  pair,
  open: async (h) => {
    const c = new RpcClient(new LinkTransport(h))
    await c.connect()
    return c
  },
  deviceName: deviceName(),
})

/** A ready phone-only node this device has not paired with yet. */
export const needsLink = (node: CloudNode | null): node is CloudNode & { link: NonNullable<CloudNode['link']> } =>
  !!node && node.state === 'ready' && !!node.link?.engine && !node.link.paired

/**
 * Pair with the node, tell the phone's engine (it forgets the PSK), then give the node's vault the master password.
 * The engine row is added to the switcher by `pair`; the caller switches to it.
 */
export async function linkNode(deps: LinkDeps, name: string, password: string): Promise<{ engine: Engine; vault: VaultOutcome }> {
  const key = await deps.api.cloud_link_pairing(name)
  const engine = await deps.pair(key, deps.deviceName)
  await deps.api.cloud_link_paired(name)
  if (!password) return { engine, vault: 'none' }
  let vault: VaultOutcome = 'failed'
  try {
    const c = await deps.open(engine)
    try {
      await vaultApi(c).addPassword(null, password)
      vault = 'set'
    } catch (e) {
      vault = isUnsupported(e) ? 'unsupported' : 'failed'
    } finally {
      c.close()
    }
  } catch {
    vault = 'failed'
  }
  return { engine, vault }
}

export type PhoneLink = { phase: 'idle' } | { phase: 'pairing' } | { phase: 'paired'; engine: Engine; vault: VaultOutcome } | { phase: 'error'; error: string }

/** Runs `linkNode` once the node is ready; `retry` after an error. `use` switches this device to the node. */
export function usePhoneLink(api: LinkDeps['api'], node: CloudNode | null, password: string, deps?: LinkDeps) {
  const [state, setState] = useState<PhoneLink>({ phase: 'idle' })
  const [attempt, setAttempt] = useState(0)
  const pw = useRef(password)
  pw.current = password
  const ready = needsLink(node)
  const name = node?.name ?? null
  // One run per node and attempt. Not cancelled when `node` changes: `cloud_link_paired` itself flips `paired`.
  const started = useRef<string | null>(null)
  useEffect(() => {
    const key = `${name}#${attempt}`
    if (!ready || !name || started.current === key) return
    started.current = key
    setState({ phase: 'pairing' })
    linkNode(deps ?? liveDeps(api), name, pw.current)
      .then((r) => setState({ phase: 'paired', ...r }))
      .catch((e: unknown) => setState({ phase: 'error', error: e instanceof Error ? e.message : String(e) }))
    // eslint-disable-next-line react-hooks/exhaustive-deps -- keyed by node and attempt
  }, [ready, name, attempt])
  const retry = () => setAttempt((n) => n + 1)
  /** Switch to the node: the one just paired, or one paired in an earlier session that is still in the switcher. */
  const use = () => {
    const id = state.phase === 'paired' ? state.engine.id : node?.link?.paired ? node.link.engine : null
    const { engines, setActive } = useEngines.getState()
    if (id && engines.some((e) => e.id === id)) setActive(id)
  }
  return { state, retry, use }
}
