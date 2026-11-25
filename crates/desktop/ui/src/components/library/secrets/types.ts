// 221 §4, §6.3, §7.3, §12: the external-secrets wire types, mirrored by hand until the generated bindings carry them
// (`crates/secrets/src/external.rs`, `crates/secrets/src/rpc.rs`). Field names and serde shapes follow the Rust:
// `snake_case`, `ConnStatus` tagged `state`/`detail`, `DeleteMode` externally tagged. Phase 2 swaps every import of
// this file for `@/rpc/bindings/method.secrets.*`; nothing else imports wire shapes.

/** The placeholder namespace and the always-ask key's backend. */
export type Backend = 'aws' | 'bw'

/** `ConnStatus` (`#[serde(tag = "state", content = "detail")]`). */
export type ConnStatus =
  | { state: 'ready' }
  | { state: 'locked' }
  | { state: 'logged_out' }
  | { state: 'expired'; detail: { hint: string } }
  | { state: 'error'; detail: string }
  | { state: 'unsupported'; detail: string }

/** `ConnInfo.ask`: which of the connection's four always-ask entries are in the global set. `send` is null for AWS. */
export interface AskInfo {
  read: boolean
  write: boolean
  delete: boolean
  send: boolean | null
}

/** D20, per connection (`secrets.connections.<conn>.…`). Defaults in `CONN_DEFAULTS`. */
export interface ConnSettings {
  send_owner_only: boolean
  allow_permanent_delete: boolean
  delete_recovery_days: number
  remember_unlock: boolean
  lock_after_idle_secs: number
  lock_on_sleep: boolean
  agent_may_request_login: boolean
  expose_password_history: boolean
  expose_totp_seed: boolean
  write_scope: 'personal' | 'personal_and_org'
  mcp_config_refs: boolean
  manage_all_sends: boolean
  send_link_to_agent: boolean
  allow_reveal: boolean
}

/** D20, harness-wide (`secrets.external.…`). */
export interface ExternalSettings {
  send_max_file_mb: number
  copy_link_needs_press: boolean
}

/** §6.3 `ConnInfo`. `summary`: `work-admin · eu-west-1` | `me@… · vault.bitwarden.eu`. */
export interface ConnInfo {
  conn: string
  backend: Backend
  status: ConnStatus
  summary: string
  ask: AskInfo
  settings: ConnSettings
}

/** `us` | `eu` | `self_hosted` (§6.2). */
export type BwServer = 'us' | 'eu' | 'self_hosted'

/** §6.1 / §6.2 `ConnConfig`, tagged by `backend` like `Location`. */
export type ConnConfig =
  | { backend: 'aws'; profile: string | null; region: string; kms_key_id: string | null; prefix: string | null }
  | {
      backend: 'bw'
      server: BwServer
      base_url: string | null
      api_url: string | null
      identity_url: string | null
      web_vault_url: string | null
      email: string
      login: 'api_key' | 'password'
    }

/** §6.3 `LoginForm`. `two_factor` = `[provider, code]`. */
export type LoginForm =
  | { method: 'api_key'; client_id: string; client_secret: string }
  | { method: 'password'; email: string; password: string; two_factor: [string, string] | null; new_device_otp: string | null }

/** §6.3 `LoginStep`. */
export type LoginStep = { step: 'done' } | { step: 'need_two_factor'; providers: string[] } | { step: 'need_new_device_otp' }

/** `ListQuery`; `deleted` lists AWS secrets pending deletion / the Bitwarden trash (§9.1's Deleted filter). */
export interface ListQuery {
  query: string | null
  cursor: string | null
  deleted: boolean
}

export interface ExtVersion {
  id: string
  stages: string[]
  changed: string | null
}

/** One entry's metadata, never a value. `ref` is the placeholder body (`bw/personal/2f1c…`, `aws/prod/db/main`). */
export interface ExtEntry {
  ref: string
  name: string
  kind: string
  keys: string[]
  location: string
  versions: ExtVersion[]
  /** Deleted entries: RFC 3339 until when Restore works (AWS purge date / Bitwarden trash); absent otherwise. */
  recoverable_until?: string | null
}

export interface ListPage {
  entries: ExtEntry[]
  next: string | null
}

/** `DeleteMode` (externally tagged, `snake_case`). */
export type DeleteMode = { recoverable: { days: number | null } } | 'permanent'

export interface DeleteOutcome {
  recoverable_until: string | null
}

/** §7.3 `Destination`: where a `secret_request` answer is stored. */
export interface Destination {
  connection: string
  name: string
  replace: string | null
  keep_local: boolean
  kms_key_id: string | null
  format: 'string' | 'json' | null
  org: string | null
  collection: string | null
  folder: string | null
  item: 'login' | 'secure_note' | null
  fields: Record<string, string> | null
  uri: string | null
}

/** §7.3 `DestinationView`: the destination with names resolved on the engine, as the request card shows it. */
export interface DestinationView extends Destination {
  backend: Backend
  /** The connection's `ConnInfo.summary`. */
  summary: string
  /** Whether the connection is locked now: the card shows the unlock field first (§9.2). */
  status: ConnStatus
  /** The entry's current revision when `replace` is set. */
  current_rev: string | null
}

/** §9.4 `SecretsEvent::UnlockRequested` (on the wire `ExternalUnlockRequested`). */
export interface UnlockRequested {
  id: string
  conn: string
  backend: Backend
  account: string
  reason: string
}

/** §12.3 `SendPassword` (untagged). */
export type SendPassword = { from: string } | { ask: boolean }

/** §12.5 `create_send(conn, args)`: `SendCreateArgs` from the owner's dialog. Exactly one of `source` / `file`. */
export interface SendArgs {
  source: string | null
  file: string | null
  as_file: string | null
  file_name: string | null
  name: string | null
  expires: string | null
  delete: string | null
  max_access_count: number | null
  password: SendPassword | null
  hide_email: boolean | null
  notes: string | null
  link_name: string | null
}

export type SendStatus = 'active' | 'expired' | 'used_up' | 'revoked' | 'deleted_by_bitwarden'

/** §12.4 `external_sends` row as `list_sends` returns it; never the link. */
export interface SendRecord {
  id: string
  conn: string
  kind: 'text' | 'file'
  name: string
  file_name: string | null
  file_bytes: number | null
  source_ref: string
  link_secret: string | null
  created_ms: number
  expiration_ms: number | null
  deletion_ms: number
  access_count: number
  max_access: number | null
  has_password: boolean
  status: SendStatus
}

/** D20 defaults: what a new connection starts with. */
export const CONN_DEFAULTS: ConnSettings = {
  send_owner_only: true,
  allow_permanent_delete: false,
  delete_recovery_days: 30,
  remember_unlock: false,
  lock_after_idle_secs: 900,
  lock_on_sleep: true,
  agent_may_request_login: false,
  expose_password_history: false,
  expose_totp_seed: false,
  write_scope: 'personal',
  mcp_config_refs: false,
  manage_all_sends: false,
  send_link_to_agent: false,
  allow_reveal: true,
}

export const EXTERNAL_DEFAULTS: ExternalSettings = { send_max_file_mb: 100, copy_link_needs_press: true }
