// 224 §16.12: a phone-only node (made by the iPhone's own engine) is paired with this device over the relay once it is
// ready, the phone's engine is told (it forgets the one-time PSK), the master password goes to the node over that link,
// and "Connect a model" switches the app to the node.
import '@/components/settings/test-dom'
import { act, createElement } from 'react'
import { type Root, createRoot } from 'react-dom/client'
import { afterEach, beforeEach, expect, test, vi } from 'vitest'
import { RpcError } from '@/rpc/client'
import { useEngines, type Engine } from '@/link/engines'
import { type LinkDeps, linkNode, needsLink, usePhoneLink } from './phoneLink'
import type { CloudNode } from './types'

vi.mock('@/link/idb', () => ({ idbGet: async () => undefined, idbPut: async () => undefined }))

const ENGINE: Engine = { id: 'node-pub', name: 'lab', relayHost: 'relay.test', secure: true, pairedAt: 1, lastSeen: 1, order: 1, kind: 'engine' }

function node(link: Partial<NonNullable<CloudNode['link']>> | null, state: CloudNode['state'] = 'ready'): CloudNode {
  return {
    name: 'lab',
    provider: 'digitalocean',
    region: 'lon1',
    region_label: 'London',
    memory_gb: 2,
    size_slug: 's-1vcpu-2gb',
    volume_gb: 10,
    monthly_usd: null,
    state,
    stage: 'ready',
    error: null,
    provider_state: null,
    ip: null,
    peer_id: null,
    reachable: null,
    path: null,
    version: null,
    auto_update: true,
    desktop: true,
    created_ms: 1,
    k8s: null,
    link: link && { claim_id: 'c', expires_ms: 9e15, engine: 'node-pub', engine_name: 'lab', paired: false, ...link },
  } as CloudNode
}

function deps(log: string[], vault: (pw: string) => Promise<unknown> = async () => ({})): LinkDeps {
  return {
    api: {
      cloud_link_pairing: vi.fn(async (name: string) => {
        log.push(`pairing ${name}`)
        return 'https://relay.test/link#1.KEY.bGFi'
      }),
      cloud_link_paired: vi.fn(async (name: string) => {
        log.push(`paired ${name}`)
        return node({ paired: true })
      }),
    } as unknown as LinkDeps['api'],
    pair: async (key, device) => {
      log.push(`pair ${key.split('#')[0]} as ${device}`)
      useEngines.getState().add(ENGINE)
      return ENGINE
    },
    open: async (h) => {
      log.push(`open ${h.id}`)
      return {
        call: async <T,>(method: string, params: unknown[]) => {
          log.push(`${method} ${JSON.stringify(params[0])}`)
          return (await vault(params[1] as string)) as T
        },
        close: () => log.push('close'),
      }
    },
    deviceName: 'iPhone',
  }
}

let root: Root | null = null
let host: HTMLElement

beforeEach(() => {
  useEngines.setState({ engines: [], active: 'local' })
})
afterEach(() => {
  act(() => root?.unmount())
  root = null
  host?.remove()
})

test('needsLink: a ready phone-only node that answered and is not paired yet', () => {
  expect(needsLink(node({}))).toBe(true)
  expect(needsLink(node({}, 'creating'))).toBe(false)
  expect(needsLink(node({ engine: null }))).toBe(false)
  expect(needsLink(node({ paired: true }))).toBe(false)
  expect(needsLink(node(null))).toBe(false)
})

test('linkNode: pairing string, pair, forget the claim, then the password on the node with engine null', async () => {
  const log: string[] = []
  const r = await linkNode(deps(log), 'lab', 'correct horse battery')
  expect(r).toEqual({ engine: ENGINE, vault: 'set' })
  expect(log).toEqual(['pairing lab', 'pair https://relay.test/link as iPhone', 'paired lab', 'open node-pub', 'vault_add_password null', 'close'])
})

test('linkNode: no password, an older node without the vault, a failing call', async () => {
  const none: string[] = []
  expect((await linkNode(deps(none), 'lab', '')).vault).toBe('none')
  expect(none.some((l) => l.startsWith('open'))).toBe(false)
  const old = await linkNode(deps([], async () => Promise.reject(new RpcError(-32601, 'method not found'))), 'lab', 'correct horse battery')
  expect(old.vault).toBe('unsupported')
  const bad = await linkNode(deps([], async () => Promise.reject(new Error('boom'))), 'lab', 'correct horse battery')
  expect(bad.vault).toBe('failed')
})

test('usePhoneLink pairs once when the node is ready, survives the paired flip, and `use` switches to the node', async () => {
  const log: string[] = []
  const d = deps(log)
  let hook: ReturnType<typeof usePhoneLink> | null = null
  function Probe({ n }: { n: CloudNode }) {
    hook = usePhoneLink(d.api, n, 'correct horse battery', d)
    return null
  }
  host = document.createElement('div')
  document.body.append(host)
  root = createRoot(host)
  await act(async () => root?.render(createElement(Probe, { n: node({}, 'creating') })))
  expect(hook!.state.phase).toBe('idle')
  await act(async () => root?.render(createElement(Probe, { n: node({}) })))
  // The list event after `cloud_link_paired` flips `paired` while the run is in flight.
  await act(async () => root?.render(createElement(Probe, { n: node({ paired: true }) })))
  // Let the in-flight run's awaits settle (each fake dep resolves on the next microtask).
  for (let i = 0; i < 10; i++) await act(async () => Promise.resolve())
  expect(hook!.state).toEqual({ phase: 'paired', engine: ENGINE, vault: 'set' })
  expect(log.filter((l) => l.startsWith('pairing'))).toHaveLength(1)
  expect(useEngines.getState().active).toBe('local')
  act(() => hook!.use())
  expect(useEngines.getState().active).toBe('node-pub')
})
