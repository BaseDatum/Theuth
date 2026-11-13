// Bundled serde receive semantics, not a validator or an interpreter of remote schemas.
export function emitAdapters(document, schemas) {
  const shapes = [], ids = new WeakMap()
  const resolve = schema => schema?.$ref ? resolve(schemas[schema.$ref.replace('#/components/schemas/', '')]) : schema
  function nullable(schema) {
    schema = resolve(schema)
    if (schema === true) return true
    if (!schema || typeof schema !== 'object') return false
    if (schema.type === 'null' || schema.type?.includes?.('null') || schema.const === null || schema.enum?.includes(null)) return true
    return schema.anyOf?.some(nullable) || schema.oneOf?.some(nullable) || schema.allOf?.every(nullable) || false
  }
  function tag(schema) {
    schema = resolve(schema)
    return JSON.stringify(Object.entries(schema?.properties ?? {}).filter(([, s]) => s && 'const' in s).map(([k, s]) => [k, s.const]))
  }
  function profile(output, receiver) {
    output = resolve(output); receiver = resolve(receiver)
    if (typeof output !== 'object' || output === null) return output === false ? -1 : 0
    if (!ids.has(output)) ids.set(output, new Map())
    const cached = ids.get(output)
    if (cached.has(receiver)) return cached.get(receiver)
    const id = shapes.length
    cached.set(receiver, id)
    const shape = {}
    shapes.push(shape)
    const input = receiver && typeof receiver === 'object' ? receiver : output
    if (output.type) shape.type = Array.isArray(output.type) ? output.type : [output.type]
    if ('const' in output) shape.values = [output.const]
    else if (output.enum) shape.values = output.enum
    if (input.required?.length) shape.required = input.required
    if (output.properties) {
      shape.properties = {}; shape.defaults = {}; shape.tags = {}
      for (const [key, property] of Object.entries(output.properties)) {
        const received = input.properties?.[key]
        shape.properties[key] = profile(property, received)
        if (received && output.required?.includes(key) && 'default' in received) shape.defaults[key] = received.default
        else if (received && output.required?.includes(key) && !input.required?.includes(key) && nullable(received)) shape.defaults[key] = null
        const scalar = resolve(property)
        if (scalar && typeof scalar === 'object' && 'const' in scalar) shape.tags[key] = [scalar.const]
      }
    }
    for (const key of ['allOf', 'anyOf', 'oneOf']) {
      if (!output[key]) continue
      const candidates = input[key] ?? []
      shape[key] = output[key].map((s, i) => profile(s, candidates.find(r => tag(s) !== '[]' && tag(r) === tag(s)) ?? (candidates.length === output[key].length ? candidates[i] : undefined)))
    }
    if (output.items) shape.items = Array.isArray(output.items) ? output.items.map((s, i) => profile(s, input.items?.[i])) : profile(output.items, input.items)
    if (output.additionalProperties && typeof output.additionalProperties === 'object') shape.additional = profile(output.additionalProperties, input.additionalProperties)
    return id
  }
  // Zero is the unconstrained, no-default profile; -1 never selects a union branch.
  shapes.push({})
  const methods = Object.fromEntries(document.methods.map(m => [m.name, profile(m.result.schema, m.result['x-theuth-receiverSchema'])]))
  const topics = Object.fromEntries(document['x-theuth'].events.map(e => [`${e.topic}@${e.version}`, profile(e.schema, e.receiverSchema)]))
  const streams = Object.fromEntries(document['x-theuth'].subscriptions.map(s => [s.subscribe, profile(s.itemSchema, s.receiverSchema)]))
  const components = Object.fromEntries(Object.keys(schemas).filter(key => key.includes('.output.') && schemas[key.replace('.output.', '.receiver.')]).map(key => [key, profile(schemas[key], schemas[key.replace('.output.', '.receiver.')])]))
  return `/* Generated from bundled output/receiver schemas; do not edit. */\nimport type { MethodMap, ReceiverResults, ComponentResults, ComponentReceivers } from './client'\n${runtime}\nconst shapes: Shape[] = [${shapes.map(shape => `${JSON.stringify(shape)} as Shape`).join(',')}]\nconst methods: Record<string, number> = ${JSON.stringify(methods)}\nconst topics: Record<string, number> = ${JSON.stringify(topics)}\nconst streams: Record<string, number> = ${JSON.stringify(streams)}\nconst components: Record<string, number> = ${JSON.stringify(components)}\n`
}

const runtime = String.raw`type Shape = {
  type?: string[]; values?: unknown[]; required?: string[]; tags?: Record<string, unknown[]>
  properties?: Record<string, number>; defaults?: Record<string, unknown>; additional?: number
  allOf?: number[]; anyOf?: number[]; oneOf?: number[]; items?: number | number[]
}
function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}
// Shallow branch selection only. Unrecognised/malformed values pass through;
// this does not validate payloads, reject extras or replace owner validators.
function matches(id: number, value: unknown): boolean {
  if (id < 0) return false
  const shape = shapes[id]!
  if (shape.values && !shape.values.includes(value)) return false
  if (shape.type && !shape.type.some(t => t === 'null' ? value === null : t === 'array' ? Array.isArray(value) : t === 'object' ? object(value) : t === 'integer' ? typeof value === 'number' : typeof value === t)) return false
  if (object(value)) {
    if (shape.required?.some(key => !Object.hasOwn(value, key))) return false
    for (const key in shape.tags) if (!shape.tags[key]!.includes(value[key])) return false
  }
  if (shape.allOf && !shape.allOf.every(i => matches(i, value))) return false
  const union = shape.oneOf ?? shape.anyOf
  return !union || union.some(i => matches(i, value))
}
function apply(id: number | undefined, value: unknown): unknown {
  if (id === undefined || id === 0 || !matches(id, value)) return value
  const shape = shapes[id]!
  if (shape.allOf) for (const i of shape.allOf) apply(i, value)
  const union = shape.oneOf ?? shape.anyOf
  if (union) {
    let selected: number | undefined
    for (const i of union) {
      if (!matches(i, value)) continue
      if (selected !== undefined) { selected = undefined; break }
      selected = i
    }
    if (selected !== undefined) apply(selected, value)
  }
  if (object(value)) {
    for (const key in shape.defaults) {
      if (Object.hasOwn(value, key) && value[key] !== undefined) continue
      const fallback = shape.defaults[key]
      // Default objects must never be shared across results or connections.
      Object.defineProperty(value, key, { value: fallback !== null && typeof fallback === 'object' ? structuredClone(fallback) : fallback, writable: true, configurable: true, enumerable: true })
    }
    for (const key in value) {
      if (!Object.hasOwn(value, key)) continue
      const child = shape.properties && Object.hasOwn(shape.properties, key) ? shape.properties[key] : shape.additional
      if (child !== undefined) value[key] = apply(child, value[key])
    }
  } else if (Array.isArray(value) && shape.items !== undefined) {
    for (let i = 0; i < value.length; i++) value[i] = apply(Array.isArray(shape.items) ? shape.items[i] : shape.items, value[i])
  }
  return value
}
export function receiveResult<M extends keyof MethodMap>(method: M, value: ReceiverResults[M]): MethodMap[M]['result']
export function receiveResult(method: string, value: unknown): unknown
export function receiveResult(method: string, value: unknown): unknown { return apply(Object.hasOwn(methods, method) ? methods[method] : undefined, value) }
export const receiveTopic = (key: string, value: unknown): unknown => apply(Object.hasOwn(topics, key) ? topics[key] : undefined, value)
export const receiveStream = (method: string, value: unknown): unknown => apply(Object.hasOwn(streams, method) ? streams[method] : undefined, value)
export function receiveComponent<K extends keyof ComponentResults, V extends ComponentReceivers[K]>(key: K, value: V): ComponentResults[K] { return apply(components[key], value) as ComponentResults[K] }
`
