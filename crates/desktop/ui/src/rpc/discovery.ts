import { z } from 'zod'
import { SUBSCRIPTIONS, type MethodMap, type TopicKey } from '@/rpc/bindings/client'

const protocol = z.object({ major: z.number().int().nonnegative(), minor: z.number().int().nonnegative() }).passthrough()
export const capabilitiesSchema = z.object({
  serverVersion: z.string(), protocolVersion: protocol,
  schemaHash: z.string().regex(/^[a-f0-9]{64}$/), plugins: z.array(z.object({ name: z.string() }).passthrough()),
}).passthrough()
const schema = z.union([z.boolean(), z.record(z.string(), z.unknown())])
export const documentSchema = z.object({
  openrpc: z.literal('1.3.2'),
  info: z.object({ title: z.string(), version: z.string() }).passthrough(),
  methods: z.array(z.object({ name: z.string(), params: z.array(z.object({ name: z.string(), required: z.boolean(), schema }).passthrough()), result: z.object({ schema, 'x-theuth-receiverSchema': schema }).passthrough() }).passthrough()),
  components: z.object({ schemas: z.record(z.string(), schema) }).passthrough(),
  'x-theuth': z.object({
    protocolVersion: protocol,
    plugins: z.array(z.string()),
    events: z.array(z.object({ topic: z.string(), version: z.number().int().positive(), schema, receiverSchema: schema }).passthrough()),
    subscriptions: z.array(z.object({ subscribe: z.string(), unsubscribe: z.string(), notification: z.string(), itemSchema: schema, receiverSchema: schema }).passthrough()),
  }).passthrough(),
}).passthrough()
export type Capabilities = z.infer<typeof capabilitiesSchema>
export type DiscoveryDocument = z.infer<typeof documentSchema>
export type Discovery = {
  phase: 'connecting' | 'discovering' | 'ready' | 'legacy' | 'failed'
  capabilities: Capabilities | null
  document: DiscoveryDocument | null
  unavailable: ReadonlySet<string>
  error?: string
}
export const disconnected = (): Discovery => ({ phase: 'connecting', capabilities: null, document: null, unavailable: new Set() })
export const ready = (d: Discovery): boolean => d.phase === 'ready' || d.phase === 'legacy'

export class UnsupportedCapability extends Error {
  constructor(readonly capability: string) {
    super(`This engine does not support ${capability}`)
    this.name = 'UnsupportedCapability'
  }
}

/** Panel-owned requirements; discovery is support metadata, never authorization. */
export type Requirements = {
  methods?: readonly (keyof MethodMap)[]
  topics?: readonly TopicKey[]
  fields?: readonly { method: keyof MethodMap; parameter?: string; path: readonly string[]; value?: string | number | boolean }[]
}
export function supports(d: Discovery, required: Requirements): boolean {
  if (!ready(d) || !d.document) return false
  const doc = d.document
  const method = (name: string) => {
    if (d.unavailable.has(name) || !doc.methods.some(m => m.name === name)) return false
    if (!Object.hasOwn(SUBSCRIPTIONS, name)) return true
    const expected = SUBSCRIPTIONS[name as keyof typeof SUBSCRIPTIONS]
    const stream = doc['x-theuth'].subscriptions.find(s => s.subscribe === name)
    return stream?.notification === expected.notification && stream.unsubscribe === expected.unsubscribe &&
      !d.unavailable.has(stream.unsubscribe) && doc.methods.some(m => m.name === stream.unsubscribe)
  }
  if (required.methods?.some(name => !method(name))) return false
  if (required.topics?.length && (!method('subscribe_topics') || required.topics.some(key => !doc['x-theuth'].events.some(e => `${e.topic}@${e.version}` === key)))) return false
  return !required.fields?.some(field => {
    if (!method(field.method)) return true
    const endpoint = doc.methods.find(m => m.name === field.method)!
    const root = field.parameter === undefined ? endpoint.result.schema : endpoint.params.find(p => p.name === field.parameter)?.schema
    return !hasField(doc, root, field.path, field.value)
  })
}
function object(value: unknown): value is Record<string, unknown> {
  return value !== null && typeof value === 'object' && !Array.isArray(value)
}
/** Field names (or `*` for array items), through refs and combiners; not a payload validator. */
function hasField(doc: DiscoveryDocument, root: unknown, path: readonly string[], value: unknown): boolean {
  const seen = new Set<string>()
  const at = (node: unknown, index: number): boolean => {
    if (node === true) return index === path.length && value === undefined
    if (!object(node)) return false
    if (typeof node.$ref === 'string') {
      const key = `${node.$ref}@${index}`
      if (!seen.has(key)) {
        seen.add(key)
        const found = at(doc.components.schemas[node.$ref.slice(21)], index)
        seen.delete(key)
        if (found) return true
      }
    }
    if (index === path.length) {
      if (value === undefined || node.const === value || (Array.isArray(node.enum) && node.enum.includes(value))) return true
    } else {
      if (object(node.properties) && Object.hasOwn(node.properties, path[index]!) && at(node.properties[path[index]!], index + 1)) return true
      if (path[index] === '*' && node.items !== undefined && (Array.isArray(node.items) ? node.items.some(s => at(s, index + 1)) : at(node.items, index + 1))) return true
    }
    return ['allOf', 'anyOf', 'oneOf'].some(key => Array.isArray(node[key]) && node[key].some(s => at(s, index)))
  }
  return at(root, 0)
}
export function validateDocument(value: unknown): DiscoveryDocument {
  const doc = documentSchema.parse(value)
  const unique = (values: string[]) => { if (new Set(values).size !== values.length) throw new Error('Duplicate discovery identity') }
  unique(doc.methods.map(m => m.name))
  unique(doc['x-theuth'].plugins)
  for (const method of doc.methods) unique(method.params.map(p => p.name))
  unique(doc['x-theuth'].events.map(e => `${e.topic}@${e.version}`))
  unique(doc['x-theuth'].subscriptions.map(s => s.subscribe))
  const refs = (s: unknown): void => {
    if (Array.isArray(s)) { s.forEach(refs); return }
    if (!object(s)) return
    if ('$ref' in s && (typeof s.$ref !== 'string' || !s.$ref.startsWith('#/components/schemas/') || !Object.hasOwn(doc.components.schemas, s.$ref.slice(21)))) throw new Error('Invalid discovery reference')
    for (const key of ['properties', 'patternProperties', 'definitions', '$defs']) {
      if (key in s && object(s[key])) Object.values(s[key]).forEach(refs)
    }
    for (const key of ['allOf', 'anyOf', 'oneOf', 'items', 'additionalProperties', 'additionalItems', 'not', 'contains', 'if', 'then', 'else']) if (key in s) refs(s[key])
  }
  Object.values(doc.components.schemas).forEach(refs)
  for (const method of doc.methods) { method.params.forEach(p => refs(p.schema)); refs(method.result.schema); refs(method.result['x-theuth-receiverSchema']) }
  for (const event of doc['x-theuth'].events) { refs(event.schema); refs(event.receiverSchema) }
  for (const sub of doc['x-theuth'].subscriptions) { refs(sub.itemSchema); refs(sub.receiverSchema) }
  return doc
}

/** Extract the already-canonical cached document without rounding JSON number tokens
 * (Rust hashes `0.0`, while JSON.parse/stringify would change it to `0`). */
export function rawResult(text: string): string {
  let depth = 0, quoted = false, escaped = false, start = -1, keyStart = -1, key = ''
  for (let i = 0; i < text.length; i++) {
    const c = text[i]
    if (quoted) {
      if (escaped) escaped = false
      else if (c === '\\') escaped = true
      else if (c === '"') { quoted = false; if (depth === 1 && start < 0) key = JSON.parse(text.slice(keyStart, i + 1)) as string }
      continue
    }
    if (start >= 0 && depth === 1 && (c === ',' || c === '}')) return text.slice(start, i).trim()
    if (c === '"') { quoted = true; keyStart = i }
    else if (c === '{' || c === '[') depth++
    else if (c === '}' || c === ']') depth--
    else if (c === ':' && depth === 1 && key === 'result') start = i + 1
  }
  throw new Error('Missing raw discovery result')
}
export async function verifyHash(raw: string, expected: string): Promise<void> {
  const bytes = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(raw))
  const hash = Array.from(new Uint8Array(bytes), b => b.toString(16).padStart(2, '0')).join('')
  if (hash !== expected) throw new Error('Discovery content hash mismatch')
}
