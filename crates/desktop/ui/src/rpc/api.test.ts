import { existsSync, readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { expect, test } from 'vitest'
import { METHODS } from './api'

// Written by `cargo test -p rpc --features ts export_bindings` (25 §7).
const METHODS_JSON = fileURLToPath(new URL('../../../../../bindings/methods.json', import.meta.url))

// Stream pairs are `RpcClient.subscribe` / `RpcClient.stream`, not `Api` methods.
const STREAM = ['subscribe_events', 'unsubscribe_events', 'subscribe_pty', 'unsubscribe_pty', 'doc_subscribe', 'doc_unsubscribe']

test.skipIf(!existsSync(METHODS_JSON))('METHODS equals the server method list', () => {
  const server = (JSON.parse(readFileSync(METHODS_JSON, 'utf8')) as string[]).filter(
    (m) => !STREAM.includes(m),
  )
  expect([...METHODS]).toEqual([...server].sort())
})

test('METHODS is sorted and unique', () => {
  expect([...METHODS]).toEqual([...new Set(METHODS)].sort())
})
