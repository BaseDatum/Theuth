// 221: what the UI needs beside the generated wire types (`@/rpc/bindings/method.secrets.*`): the backend ids the
// pickers offer, `create_send`'s argument (an untyped `serde_json::Value` on the wire, `SendCreateArgs` without
// `connection` in `crates/secrets/src/tools_send.rs`), and the D20 defaults shown before the engine answers.

import type { ConnSettings, ExternalSettings } from '@/rpc/bindings/method.secrets.output'

/** The placeholder namespaces of the backends theuth ships (`ConnInfo.backend` is a plain string on the wire). */
export type Backend = 'aws' | 'bw'

/** `SendPassword` (untagged): a vault secret, or asked on a masked card after Create. */
export type SendPassword = { from: string } | { ask: boolean }

/** §12.5 `create_send(conn, args)`: `SendCreateArgs` without `connection`. Exactly one of `source` / `file`. */
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

export const EXTERNAL_DEFAULTS: ExternalSettings = { send_max_file_mb: 100, copy_link_needs_press: true, timeout_secs: 20 }
