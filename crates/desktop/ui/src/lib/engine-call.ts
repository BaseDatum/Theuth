// 227: Settings of another machine of the mesh. `forEngine(api, target)` is an `Api` whose every method runs on
// `target` through the connected engine's `engine_call`, so a tab written against `useApi()` edits that machine as is.

import { receiveResult } from '@/rpc/bindings/receive'
import { PARAM_NAMES } from '@/rpc/bindings/param-names'
import type { Api } from '@/rpc/bindings/client'
import { RpcError } from '@/rpc/client'
import type { PressToken } from '@/rpc/bindings/method.agent.input'

/** 227 §1–2: the JSON-RPC errors of `engine_call` (and the statuses of `engine_call_all`). */
export const ENGINE_CALL_ERROR = { notForwardable: -32050, offline: -32051, sealed: -32052, notApplicable: -32053, needsPress: -32054 } as const

export const isEngineCallError = (e: unknown, kind: keyof typeof ENGINE_CALL_ERROR): boolean => e instanceof RpcError && e.code === ENGINE_CALL_ERROR[kind]

/** `method`'s parameter names in order (the bindings' `PARAM_NAMES`). */
export function paramNames(method: string): readonly string[] | undefined {
  return PARAM_NAMES[method]
}

/**
 * A positional call's arguments as named params: `set_model(null, 'x', true)` → `{ session: null, model: 'x', manual: true }`.
 * Trailing `undefined`s (omitted optionals) are left out. Throws for a method the bindings do not know, or too many arguments.
 */
export function namedParams(method: string, args: readonly unknown[]): Record<string, unknown> {
  const names = paramNames(method)
  if (!names) throw new Error(`${method} is not a known method`)
  if (args.length > names.length) throw new Error(`${method} takes ${names.length} parameters, got ${args.length}`)
  const params: Record<string, unknown> = {}
  args.forEach((value, i) => {
    if (value !== undefined) params[names[i]!] = value
  })
  return params
}

/**
 * Whether the call now running began with the user's press (a click or key, still inside its transient activation).
 * The UI claims it only then; the hub checks its own `human_press` before passing it on (227 §1).
 */
export function userPressed(): PressToken | null {
  return typeof navigator !== 'undefined' && navigator.userActivation?.isActive ? { at: Date.now() } : null
}

/**
 * The vault methods (feat/sealed-vault) take the engine as their first parameter (`null`: this one) and forward it
 * themselves; they are never sent through `engine_call`.
 */
const ownsEngineParam = (method: string) => method.startsWith('vault_')

/**
 * `api` aimed at `target` (a mesh engine id). The engine the client is connected to (`active`) is `api` itself;
 * any other target gets a Proxy whose every method `m(...args)` is `api.engine_call(target, m, named, pressed)`,
 * its answer adapted as `m`'s own would be. `pressed` defaults to `userPressed()` at call time. A vault method instead
 * runs on the connected engine with `target` as its `engine` argument.
 */
export function forEngine(api: Api, target: string, active: string, pressed: () => PressToken | null = userPressed): Api {
  if (target === active) return api
  const methods = new Map<string, (...args: unknown[]) => Promise<unknown>>()
  return new Proxy(api, {
    get(base, prop, receiver) {
      if (typeof prop !== 'string') return Reflect.get(base, prop, receiver)
      const vault = ownsEngineParam(prop) && typeof Reflect.get(base, prop, receiver) === 'function'
      if (!vault && !paramNames(prop)) return Reflect.get(base, prop, receiver)
      let m = methods.get(prop)
      if (!m) {
        const own = Reflect.get(base, prop, receiver) as (...args: unknown[]) => Promise<unknown>
        m = vault
          ? (engine: unknown, ...rest: unknown[]) => own.call(base, engine ?? target, ...rest)
          : async (...args: unknown[]) => receiveResult(prop, await base.engine_call(target, prop, namedParams(prop, args), pressed()))
        methods.set(prop, m)
      }
      return m
    },
  }) as Api
}
