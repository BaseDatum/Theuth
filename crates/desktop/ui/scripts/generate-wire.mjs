import { readFile, writeFile, mkdir, rm } from 'node:fs/promises'
import { resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { compile } from 'json-schema-to-typescript'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..')
const input = resolve(process.argv[2] ?? `${root}/bindings/openrpc.json`)
const out = resolve(process.argv[3] ?? `${root}/bindings/typescript`)
const doc = JSON.parse(await readFile(input, 'utf8')) // Missing input is always an error.
if (doc.openrpc !== '1.3.2' || !doc['x-theuth']?.events) throw new Error('Expected the composed OpenRPC 1.3.2 contract')
const schemas = structuredClone(doc.components.schemas)
const scope = key => key.split('.').slice(0, 3).join('.')
const name = key => key.split('.').slice(3).join('.')
const identifier = value => value.replace(/[^a-zA-Z0-9_]/g, '_')
const groups = new Map()
for (const [key, schema] of Object.entries(schemas)) {
  const group = scope(key)
  if (!groups.has(group)) groups.set(group, {})
  groups.get(group)[name(key)] = schema
}
function add(group, key, schema) {
  if (!groups.has(group)) groups.set(group, {})
  groups.get(group)[key] = schema
}
function params(method) {
  const items = method.params.map(p => p.schema)
  const lastRequired = method.params.findLastIndex(p => p.required)
  return { type: 'array', items, minItems: lastRequired + 1, maxItems: items.length, additionalItems: false }
}
const methods = doc.methods.filter(m => !m['x-theuth-aliasOf'])
const methodTypes = []
for (const method of methods) {
  const owner = method['x-theuth-owner'].replace(/[^a-zA-Z0-9]/g, c => `_${c.charCodeAt(0).toString(16)}`)
  const base = identifier(method.name)
  const args = `method.${owner}.input.${base}Params`
  const result = `method.${owner}.receiver.${base}Result`
  add(scope(args), name(args), params(method))
  add(scope(result), name(result), method.result['x-theuth-receiverSchema'])
  methodTypes.push([method.name, args, result])
}
for (const subscription of doc['x-theuth'].subscriptions) {
  const method = methods.find(m => m.name === subscription.subscribe)
  const owner = method['x-theuth-owner'].replace(/[^a-zA-Z0-9]/g, c => `_${c.charCodeAt(0).toString(16)}`)
  add(`method.${owner}.receiver`, `${identifier(subscription.subscribe)}Item`, subscription.receiverSchema)
}
function rewrite(value, group) {
  if (Array.isArray(value)) return value.map(v => rewrite(v, group))
  if (!value || typeof value !== 'object') return value
  const result = {}
  for (const [key, child] of Object.entries(value)) {
    if (key === '$ref') {
      const target = child.replace('#/components/schemas/', '')
      if (!schemas[target] || scope(target) !== group) throw new Error(`Invalid or cross-domain ref: ${child}`)
      result[key] = `#/definitions/${name(target)}`
    } else if (key !== 'title') result[key] = rewrite(child, group)
  }
  return result
}
await rm(out, { recursive: true, force: true })
await mkdir(out, { recursive: true })
for (const [group, definitions] of [...groups].sort(([a], [b]) => a.localeCompare(b))) {
  const defs = Object.fromEntries(Object.entries(definitions).map(([key, schema]) => [key, { ...(typeof schema === 'boolean' ? { anyOf: [schema] } : rewrite(schema, group)), title: key }]))
  const schema = { type: 'object', additionalProperties: false, required: Object.keys(defs), properties: Object.fromEntries(Object.keys(defs).map(k => [k, { $ref: `#/definitions/${k}` }])), definitions: defs }
  const text = await compile(schema, 'Contract', { bannerComment: '/* Generated from OpenRPC; do not edit. */', additionalProperties: false, unknownAny: false, unreachableDefinitions: true, format: false, maxItems: -1 })
  await writeFile(`${out}/${group}.ts`, text)
}
const imports = new Map()
function type(key) {
  const group = scope(key)
  if (!imports.has(group)) imports.set(group, `D${imports.size}`)
  return `${imports.get(group)}.Contract[${JSON.stringify(name(key))}]`
}
function schemaType(schema) {
  if (!schema?.$ref) throw new Error('Expected a named event root')
  return type(schema.$ref.replace('#/components/schemas/', ''))
}
function deref(schema) { return schema?.$ref ? deref(schemas[schema.$ref.replace('#/components/schemas/', '')]) : schema }
function variants(schema) {
  schema = deref(schema)
  if (!schema || typeof schema !== 'object') return []
  const tag = deref(schema.properties?.type)
  return [...(tag?.const ? [tag.const] : tag?.enum ?? []), ...(schema.oneOf ?? schema.anyOf ?? []).flatMap(variants)]
}
const signatures = methodTypes.map(([method, args, result]) => `  ${JSON.stringify(method)}: { params: ${type(args)}; result: ${type(result)} }`).join('\n')
const topics = doc['x-theuth'].events.map(e => `  ${JSON.stringify(`${e.topic}@${e.version}`)}: ${schemaType(e.receiverSchema)}`).join('\n')
const metadata = Object.fromEntries(doc['x-theuth'].events.map(e => [`${e.topic}@${e.version}`, variants(e.schema)]))
const subscriptions = Object.fromEntries(doc['x-theuth'].subscriptions.map(s => [s.subscribe, { unsubscribe: s.unsubscribe, notification: s.notification }]))
const items = doc['x-theuth'].subscriptions.map(s => {
  const method = methods.find(m => m.name === s.subscribe)
  const owner = method['x-theuth-owner'].replace(/[^a-zA-Z0-9]/g, c => `_${c.charCodeAt(0).toString(16)}`)
  return `  ${JSON.stringify(s.subscribe)}: ${type(`method.${owner}.receiver.${identifier(s.subscribe)}Item`)}`
}).join('\n')
const body = `export interface MethodMap {\n${signatures}\n}\nexport type Api = { [M in keyof MethodMap]: (...args: MethodMap[M]['params']) => Promise<MethodMap[M]['result']> }\nexport const METHODS = ${JSON.stringify(methodTypes.map(([m]) => m))} as const\nexport function makeApi(client: { call<T>(method: string, params: unknown[]): Promise<T> }): Api {\n  return Object.fromEntries(METHODS.map(method => [method, (...args: unknown[]) => client.call(method, args)])) as unknown as Api\n}\nexport interface TopicPayloads {\n${topics}\n}\nexport type TopicKey = keyof TopicPayloads\nexport type TopicEvent = { [K in TopicKey]: { key: K; event: TopicPayloads[K] } }[TopicKey]\nexport const TOPICS = ${JSON.stringify(metadata)} as const\nexport const SUBSCRIPTIONS = ${JSON.stringify(subscriptions)} as const\nexport interface SubscriptionItems {\n${items}\n}\n`
await writeFile(`${out}/client.ts`, `/* Generated from OpenRPC; do not edit. */\n${[...imports].map(([group, alias]) => `import type * as ${alias} from './${group}'`).join('\n')}\n${body}`)
await writeFile(`${out}/components.json`, JSON.stringify(Object.fromEntries([...groups].flatMap(([group, defs]) => Object.keys(defs).map(n => [`${group}.${n}`, { path: group, name: identifier(n) }]))), null, 2))
console.log(`Generated ${methods.length} methods, ${doc['x-theuth'].events.length} topics, ${groups.size} scoped modules from ${input}`)
