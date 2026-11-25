// 221 §6.3, §9, §12.5: the one place the external-secrets UI reaches the engine. Phase 1 calls the methods by name on
// the raw client because the generated `Api` does not carry them yet; phase 2 swaps `useSecretsApi` for `useApi()` and
// `useSecretsSupport` for `useQuerySupport`, and this file is the whole change.

import type { RpcClient } from '@/rpc/client'
import { supports } from '@/rpc/discovery'
import { useConnection } from '@/store/connection'
import type {
  ConnConfig,
  ConnInfo,
  ConnSettings,
  ConnStatus,
  DeleteMode,
  DeleteOutcome,
  Destination,
  DestinationView,
  ExternalSettings,
  ExtVersion,
  ListPage,
  ListQuery,
  LoginForm,
  LoginStep,
  SendArgs,
  SendRecord,
} from './types'

/** What the desktop shell reports for `lock_on_sleep` (`crates/desktop/src/power.rs`). */
export type PowerEvent = 'sleep' | 'screen_lock'

/** The §6.3 owner methods (and the few the UI needs beside them), typed. */
export interface SecretsApi {
  list_secret_connections(): Promise<ConnInfo[]>
  /** AWS: `ListSecrets` max 1 with the profile and region; Bitwarden: prelogin for the email (§9.1 "Test"). */
  probe_secret_connection(conn: string, cfg: ConnConfig): Promise<ConnStatus>
  add_secret_connection(conn: string, cfg: ConnConfig): Promise<ConnInfo>
  remove_secret_connection(conn: string): Promise<void>
  connection_login(conn: string, form: LoginForm): Promise<LoginStep>
  connection_unlock(conn: string, password: string): Promise<ConnInfo>
  connection_lock(conn: string): Promise<ConnInfo>
  list_external_secrets(conn: string, q: ListQuery): Promise<ListPage>
  answer_unlock(id: string, password: string | null): Promise<boolean>
  set_connection_setting<K extends keyof ConnSettings>(conn: string, key: K, value: ConnSettings[K]): Promise<void>
  get_external_settings(): Promise<ExternalSettings>
  set_external_setting<K extends keyof ExternalSettings>(key: K, value: ExternalSettings[K]): Promise<void>
  delete_external_secret(conn: string, id: string, mode: DeleteMode): Promise<DeleteOutcome>
  restore_external_secret(conn: string, id: string): Promise<ExtVersion>
  reveal_external_secret(conn: string, ref: string): Promise<string>
  create_send(conn: string, args: SendArgs): Promise<SendRecord>
  list_sends(conn: string): Promise<SendRecord[]>
  revoke_send(id: string): Promise<void>
  copy_send_link(id: string): Promise<string>
  /** `lock_on_sleep` (D20): the engine locks every connection with the setting on. */
  secrets_power_event(kind: PowerEvent): Promise<void>
  /** 37's `answer_secret` with §9.2's optional tail `destination`. */
  answer_secret(id: string, value: string | null, values: Record<string, string> | null, expires_ms: number | null, destination: Destination | null): Promise<boolean>
  /** 219's harness-wide always-ask switch (`session` null). */
  set_always_ask(session: null, tool: string, on: boolean): Promise<void>
}

/** Method names the Connections section needs before it renders. */
export const CONNECTION_METHODS = ['list_secret_connections', 'add_secret_connection', 'remove_secret_connection', 'connection_unlock', 'connection_lock', 'list_external_secrets', 'set_connection_setting'] as const

const NAMES = [
  'list_secret_connections', 'probe_secret_connection', 'add_secret_connection', 'remove_secret_connection', 'connection_login',
  'connection_unlock', 'connection_lock', 'list_external_secrets', 'answer_unlock', 'set_connection_setting', 'get_external_settings',
  'set_external_setting', 'delete_external_secret', 'restore_external_secret', 'reveal_external_secret', 'create_send', 'list_sends',
  'revoke_send', 'copy_send_link', 'secrets_power_event', 'answer_secret', 'set_always_ask',
] as const satisfies readonly (keyof SecretsApi)[]

/** Positional JSON-RPC calls on `client`, one per method. */
export function secretsApi(client: Pick<RpcClient, 'call'>): SecretsApi {
  return Object.fromEntries(NAMES.map((m) => [m, (...args: unknown[]) => client.call(m, args)])) as unknown as SecretsApi
}

/** Calls on whatever client the connection holds when the call is made; it rejects before one opened. */
const LIVE = secretsApi({
  call: <T,>(method: string, params: unknown[]): Promise<T> => {
    const client = useConnection.getState().client
    return client ? client.call<T>(method, params) : Promise.reject(new Error(`${method} before the connection opened`))
  },
})

/** The connected engine's `SecretsApi` (one stable object; it reads the live client per call). */
export function useSecretsApi(): SecretsApi {
  return LIVE
}

/**
 * §7.3: a `secret_requests` row's or a `SecretRequested` event's `destination`. The generated row and event types do
 * not carry the field yet; phase 2 reads `row.destination` directly.
 */
export function destinationOf(request: object | null | undefined): DestinationView | null {
  return (request as { destination?: DestinationView | null } | null | undefined)?.destination ?? null
}

/** Whether the engine serves every one of `methods` (an engine without 221 hides the sections). */
export function useSecretsSupport(methods: readonly string[]): boolean {
  return useConnection((s) => s.discovery.phase === 'legacy' || supports(s.discovery, { methods: methods as never }))
}

/** Query keys of the metadata the page caches. Values (Reveal) are never cached: see `Reveal.tsx`. */
export const secretKeys = {
  connections: ['secret-connections'] as const,
  external: (conn: string, query: string, deleted: boolean) => ['secret-external', conn, query, deleted] as const,
  externalAll: ['secret-external'] as const,
  sends: (conn: string) => ['secret-sends', conn] as const,
  settings: ['secret-external-settings'] as const,
}
