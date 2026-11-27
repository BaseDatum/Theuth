// 227 §1: the four engine RPCs of per-machine Settings, typed by hand from the contract until the engine's wire
// commit lands them in the generated bindings. Then delete this file and import the generated types instead.

import type { Api } from '@/rpc/bindings/client'

/** Proof that a call began with an owner's press in the UI (ms epoch of the press); advisory: the connected engine checks its own `human_press` before forwarding it. */
export type PressToken = { at: number }

/** One engine's outcome of `engine_call_all`. */
export type NodeStatus = 'ok' | 'failed' | 'offline' | 'sealed' | 'not_applicable'

export type NodeResult = {
  engine: string
  name: string
  status: NodeStatus
  message?: string | null
  /** The method changed something the engine reads only at start (`config_flag_set` of `plugin/*.enabled`, `subsystems.*`). */
  restart_required: boolean
  value?: unknown
}

/** `engine_caps()`: what an engine's host has. */
export type WireEngineCaps = {
  engine: string
  name: string
  platform: string
  display: boolean
  mic: boolean
  interactive_user: boolean
  headless: boolean
  hub: boolean
}

/** `settings_values`: per engine, per getter method, its answer (`null` when the getter failed there). */
export type SettingsValues = Record<string, Record<string, unknown>>

export type EngineCallApi = {
  engine_call(engine: string, method: string, params: Record<string, unknown>, pressed?: PressToken | null): Promise<unknown>
  engine_call_all(method: string, params: Record<string, unknown>, engines?: string[] | null, pressed?: PressToken | null): Promise<NodeResult[]>
  engine_caps(): Promise<WireEngineCaps>
  settings_values(engines: string[], keys: string[]): Promise<SettingsValues>
}

/** The parameter names of the four (the contract's), for named-param forwarding of `engine_caps` and friends. */
export const ENGINE_CALL_PARAM_NAMES: Readonly<Record<keyof EngineCallApi, readonly string[]>> = {
  engine_call: ['engine', 'method', 'params', 'pressed'],
  engine_call_all: ['method', 'params', 'engines', 'pressed'],
  engine_caps: [],
  settings_values: ['engines', 'keys'],
}

/** The generated `Api` with the four; generic calls reach them while the bindings lack them. */
export type FullApi = Api & EngineCallApi

/** The four on `api`: `makeApi` builds methods from `METHODS` only, so until the bindings carry them they go through the raw client. */
export function engineCallApi(api: Api, call?: (method: string, params: unknown[]) => Promise<unknown>): FullApi {
  const own = api as Partial<FullApi>
  if (own.engine_call || !call) return api as FullApi
  const via = (method: keyof EngineCallApi) => (...args: unknown[]) => call(method, args)
  return Object.assign(Object.create(api) as FullApi, {
    engine_call: via('engine_call'),
    engine_call_all: via('engine_call_all'),
    engine_caps: via('engine_caps'),
    settings_values: via('settings_values'),
  })
}
