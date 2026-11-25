// 221 §9.6 Bitwarden "Log in with SSO" on the desktop: the shell binds a one-shot loopback listener on a port of
// 8065–8070 (`crates/desktop/src/sso.rs`), `connection_sso_begin` builds the authorize URL for that port, the system
// browser opens it, the listener hands back the redirect URL, and `connection_sso_complete` exchanges it. A
// `need_two_factor` answer is retried with the same state and URL plus the code. Engine errors are shown verbatim.

import { invoke } from '@tauri-apps/api/core'
import { useEffect, useRef, useState } from 'react'
import { errorText } from '@/components/settings/shared'
import { openExternal } from '@/lib/platform'
import type { LoginStep } from '@/rpc/bindings/method.secrets.output'
import { useConnection } from '@/store/connection'

/** `connection_sso_begin`'s answer. */
export type SsoBegin = { authorize_url: string; state: string; redirect_port: number }

/** What the flow needs from the shell and the engine; injected by tests. */
export type SsoDeps = {
  listen(): Promise<number>
  wait(port: number): Promise<string>
  cancel(): Promise<void>
  open(url: string): void
  begin(conn: string, org: string | null, port: number): Promise<SsoBegin>
  complete(conn: string, state: string, redirect: string, twoFactor: [string, string] | null): Promise<LoginStep>
}

function call<T>(method: string, params: unknown[]): Promise<T> {
  const client = useConnection.getState().client
  return client ? client.call<T>(method, params) : Promise.reject(new Error('not connected'))
}

/** The real shell and engine. */
export const SSO_DEPS: SsoDeps = {
  listen: () => invoke<number>('sso_listen'),
  wait: (port) => invoke<string>('sso_wait', { port }),
  cancel: () => invoke<void>('sso_cancel'),
  open: openExternal,
  begin: (conn, org, port) => call<SsoBegin>('connection_sso_begin', [conn, org, port]),
  complete: (conn, state, redirect, twoFactor) => call<LoginStep>('connection_sso_complete', [conn, state, redirect, twoFactor]),
}

export type SsoPhase =
  | { phase: 'idle' }
  | { phase: 'browser' }
  | { phase: 'completing' }
  | { phase: 'two_factor'; providers: string[] }
  | { phase: 'done' }

/**
 * One SSO login for `conn`. `start` runs listener → begin → browser → wait → complete; `twoFactor` answers a
 * `need_two_factor`; `cancel` (and unmounting) closes the listener. A cancelled or superseded run never writes state.
 */
export function useSsoLogin(conn: string, onDone: () => void, deps: SsoDeps = SSO_DEPS) {
  const [state, setState] = useState<SsoPhase>({ phase: 'idle' })
  const [error, setError] = useState<string | null>(null)
  const run = useRef(0)
  const d = useRef(deps)
  d.current = deps
  // The answer a 2FA retry repeats: the begin's state and the redirect URL.
  const answer = useRef<{ state: string; redirect: string } | null>(null)
  const live = useRef(false)
  const close = () => {
    if (!live.current) return
    live.current = false
    void d.current.cancel().catch(() => undefined)
  }
  useEffect(
    () => () => {
      run.current++
      close()
    },
    [],
  )
  const settle = (seq: number, step: LoginStep, providers: string[]) => {
    if (seq !== run.current) return
    if (step.step === 'need_two_factor') setState({ phase: 'two_factor', providers: step.providers })
    else if (step.step === 'done') {
      answer.current = null
      setState({ phase: 'done' })
      onDone()
    } else {
      // §9.6: new-device verification never applies to SSO.
      setError(`unexpected login step ${step.step}`)
      setState(providers.length ? { phase: 'two_factor', providers } : { phase: 'idle' })
    }
  }
  const start = async (org: string | null) => {
    const seq = ++run.current
    setError(null)
    answer.current = null
    setState({ phase: 'browser' })
    try {
      const port = await d.current.listen()
      live.current = true
      // Cancelled while the shell was binding: close what it bound.
      if (seq !== run.current) return close()
      const begun = await d.current.begin(conn, org, port)
      if (seq !== run.current) return
      d.current.open(begun.authorize_url)
      const redirect = await d.current.wait(port)
      live.current = false
      if (seq !== run.current) return
      answer.current = { state: begun.state, redirect }
      setState({ phase: 'completing' })
      settle(seq, await d.current.complete(conn, begun.state, redirect, null), [])
    } catch (e) {
      if (seq !== run.current) return
      close()
      setError(errorText(e))
      setState({ phase: 'idle' })
    }
  }
  const twoFactor = async (provider: string, code: string) => {
    const a = answer.current
    if (!a || state.phase !== 'two_factor') return
    const { providers } = state
    const seq = run.current
    setError(null)
    setState({ phase: 'completing' })
    try {
      settle(seq, await d.current.complete(conn, a.state, a.redirect, [provider, code]), providers)
    } catch (e) {
      if (seq !== run.current) return
      // A wrong code may be retried with the same answer.
      setError(errorText(e))
      setState({ phase: 'two_factor', providers })
    }
  }
  const cancel = () => {
    run.current++
    answer.current = null
    setError(null)
    setState({ phase: 'idle' })
    close()
  }
  return { state, error, start, twoFactor, cancel }
}
