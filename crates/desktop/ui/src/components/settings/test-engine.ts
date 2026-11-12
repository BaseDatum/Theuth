import { createHash } from 'node:crypto'
import { readFileSync } from 'node:fs'
import { useStore } from 'zustand'
import { capabilitiesSchema, documentSchema, validateDocument } from '@/rpc/discovery'
import type { ConnectionApi } from '@/store/connection'

const raw = JSON.parse(readFileSync(new URL('../../../../../../bindings/openrpc.json', import.meta.url), 'utf8'))
const document = documentSchema.parse(raw)
validateDocument(document)
const capabilities = capabilitiesSchema.parse({
  serverVersion: document.info.version,
  protocolVersion: document['x-theuth'].protocolVersion,
  schemaHash: createHash('sha256').update(JSON.stringify(raw)).digest('hex'),
  plugins: document['x-theuth'].plugins.map(name => ({ name })),
})

/** A connected fixture backed by the exported contract, including the SSR initial snapshot. */
export function readyConnection(store: ConnectionApi) {
  store.setState({
    state: 'open',
    discovery: { phase: 'ready', capabilities, document, unavailable: new Set() },
  })
  const initial = store.getState()
  const readyStore = { ...store, getInitialState: () => initial }
  return Object.assign(<T,>(select: (state: typeof initial) => T) => useStore(readyStore, select), readyStore)
}
