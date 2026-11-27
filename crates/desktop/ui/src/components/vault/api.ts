/**
 * 225: the sealed vault's RPC surface (crates/secrets/src/rpc.rs, owner audience), called by name through the connected
 * engine's raw client until the generated `Api` carries these methods; swapping to it touches only this file.
 * Positional params; `engine` is a mesh engine id, or `null` for the engine this client is connected to.
 *
 * - `vault_status(engine)` → `VaultStatus`
 * - `vault_unseal(engine, password)` → `VaultStatus` (wrong password: `-32042`)
 * - `vault_seal(engine)` → `VaultStatus`
 * - `vault_add_password(engine, password)` → `VaultStatus`
 * - `vault_change_password(engine, old, new)` → `VaultStatus`
 * - `vault_remove_method(engine, id)` → `VaultStatus` (refuses the last method)
 * - `vault_add_device(engine, owner)` → `VaultStatus` (`owner` becomes an unlock device of `engine`)
 * - `vault_unseal_requests()` → `UnsealRequestView[]` (poll ~3 s)
 * - `vault_answer_unseal(id, approve, always)` → `boolean` (false: already answered or expired)
 * - `vault_unlock_devices()` → `UnlockDevice[]`
 * - `vault_set_auto_unlock(node, auto)` → `null`
 * - `vault_forget_node(node)` → `null`
 */
import { useMemo } from 'react'
import { RpcError, type RpcClient } from '@/rpc/client'
import { type Requirements, UnsupportedCapability } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'

export type VaultState = 'unsealed' | 'sealed' | 'unavailable'
export type VaultMethodKind = 'keychain' | 'password' | 'device' | 'file'

/** One way to unwrap the vault key. `id`: `keychain` | `password` | `file` | the owner engine id (`device`). */
export interface VaultMethod {
  kind: VaultMethodKind
  id: string
  label: string
  created_ms: number
}

export interface VaultStatus {
  /** This engine's mesh id when known. */
  engine: string | null
  name: string | null
  state: VaultState
  unsealed_by: VaultMethodKind | 'memory' | null
  methods: VaultMethod[]
  password_set: boolean
  warning: string | null
  /** Owner engine ids a sealed engine is currently asking to unlock it. */
  asking: string[]
  /** Why it is sealed or unavailable, plain text. */
  error: string | null
}

/** A pending prompt on this (owner) engine. */
export interface UnsealRequestView {
  id: string
  node: string
  node_name: string
  purpose: 'unlock' | 'enroll'
  created_ms: number
}

/** A node this engine can unlock. */
export interface UnlockDevice {
  node: string
  name: string
  auto: boolean
  enrolled_ms: number
}

/** `vault_*` errors with their own handling. */
export const VAULT_SEALED = -32041
export const WRONG_PASSWORD = -32042

export const VAULT_METHODS = [
  'vault_status',
  'vault_unseal',
  'vault_seal',
  'vault_add_password',
  'vault_change_password',
  'vault_remove_method',
  'vault_add_device',
  'vault_unseal_requests',
  'vault_answer_unseal',
  'vault_unlock_devices',
  'vault_set_auto_unlock',
  'vault_forget_node',
] as const

/** The Settings › Security › Vault tab's requirements; the cast goes once the generated `MethodMap` has the names. */
export const vaultRequirements: Requirements = { methods: VAULT_METHODS as unknown as Requirements['methods'], topics: [] }

/** The twelve methods, one function each, params in contract order. */
export interface VaultApi {
  status(engine: string | null): Promise<VaultStatus>
  unseal(engine: string | null, password: string): Promise<VaultStatus>
  seal(engine: string | null): Promise<VaultStatus>
  addPassword(engine: string | null, password: string): Promise<VaultStatus>
  changePassword(engine: string | null, old: string, next: string): Promise<VaultStatus>
  removeMethod(engine: string | null, id: string): Promise<VaultStatus>
  addDevice(engine: string | null, owner: string): Promise<VaultStatus>
  unsealRequests(): Promise<UnsealRequestView[]>
  answerUnseal(id: string, approve: boolean, always: boolean): Promise<boolean>
  unlockDevices(): Promise<UnlockDevice[]>
  setAutoUnlock(node: string, auto: boolean): Promise<null>
  forgetNode(node: string): Promise<null>
}

/** The twelve methods over `client` (anything with the raw `call`). */
export function vaultApi(client: Pick<RpcClient, 'call'>): VaultApi {
  const call = <T>(method: (typeof VAULT_METHODS)[number], params: unknown[]) => client.call<T>(method, params)
  return {
    status: (engine: string | null) => call<VaultStatus>('vault_status', [engine]),
    unseal: (engine: string | null, password: string) => call<VaultStatus>('vault_unseal', [engine, password]),
    seal: (engine: string | null) => call<VaultStatus>('vault_seal', [engine]),
    addPassword: (engine: string | null, password: string) => call<VaultStatus>('vault_add_password', [engine, password]),
    changePassword: (engine: string | null, old: string, next: string) => call<VaultStatus>('vault_change_password', [engine, old, next]),
    removeMethod: (engine: string | null, id: string) => call<VaultStatus>('vault_remove_method', [engine, id]),
    addDevice: (engine: string | null, owner: string) => call<VaultStatus>('vault_add_device', [engine, owner]),
    unsealRequests: () => call<UnsealRequestView[]>('vault_unseal_requests', []),
    answerUnseal: (id: string, approve: boolean, always: boolean) => call<boolean>('vault_answer_unseal', [id, approve, always]),
    unlockDevices: () => call<UnlockDevice[]>('vault_unlock_devices', []),
    setAutoUnlock: (node: string, auto: boolean) => call<null>('vault_set_auto_unlock', [node, auto]),
    forgetNode: (node: string) => call<null>('vault_forget_node', [node]),
  }
}

/** The connected engine's vault methods. Routes render only once connected, so a missing client is a bug. */
export function useVaultApi(): VaultApi {
  const client = useConnection((s) => s.client)
  if (!client) throw new Error('useVaultApi before the connection opened')
  return useMemo(() => vaultApi(client), [client])
}

/** The engine has no vault methods (older engine): discovery says so, or the call came back method-not-found. */
export const isUnsupported = (e: unknown): boolean => e instanceof UnsupportedCapability || (e instanceof RpcError && e.code === -32601)

export const isWrongPassword = (e: unknown): boolean => e instanceof RpcError && e.code === WRONG_PASSWORD

/** Query keys, per engine id (`null`: the connected engine). */
export const vaultKeys = {
  all: ['vault'] as const,
  status: (engine: string | null) => ['vault', 'status', engine] as const,
  devices: ['vault', 'devices'] as const,
}
