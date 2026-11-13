import { readFile, writeFile, mkdir, rm } from 'node:fs/promises'
import { resolve, dirname } from 'node:path'
import { fileURLToPath } from 'node:url'
import { compile } from 'json-schema-to-typescript'
import { toSafeString } from 'json-schema-to-typescript/dist/src/utils.js'
import { emitAdapters } from './receive-adapters.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '../../../..')
const input = resolve(process.argv[2] ?? `${root}/bindings/openrpc.json`)
const out = resolve(process.argv[3] ?? `${root}/bindings/typescript`)
const doc = JSON.parse(await readFile(input, 'utf8')) // Missing input is always an error.
if (doc.openrpc !== '1.3.2' || !doc['x-theuth']?.events) throw new Error('Expected the composed OpenRPC 1.3.2 contract')
const schemas = structuredClone(doc.components.schemas)
const scope = key => key.split('.').slice(0, 3).join('.')
const name = key => key.split('.').slice(3).join('.')
const auxiliary = JSON.parse(await readFile(resolve(dirname(input), 'client-auxiliary.json'), 'utf8'))
for (const [owner, schema] of Object.entries(auxiliary)) {
  const group = `auxiliary.${owner}.output`
  const rootName = schema.title ?? 'Root'
  function local(value) {
    if (Array.isArray(value)) return value.map(local)
    if (!value || typeof value !== 'object') return value
    return Object.fromEntries(Object.entries(value).filter(([k]) => k !== 'definitions' && k !== '$schema').map(([k, v]) =>
      [k, k === '$ref' ? `#/components/schemas/${group}.${v === '#' ? rootName : v.replace('#/definitions/', '')}` : local(v)]))
  }
  schemas[`${group}.${rootName}`] = local(schema)
  for (const [key, value] of Object.entries(schema.definitions ?? {})) schemas[`${group}.${key}`] = local(value)
}
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
  const result = `method.${owner}.output.${base}Result`
  const receiver = `method.${owner}.receiver.${base}Result`
  add(scope(args), name(args), params(method))
  add(scope(result), name(result), method.result.schema)
  add(scope(receiver), name(receiver), method.result['x-theuth-receiverSchema'])
  methodTypes.push([method.name, args, result, receiver])
}
for (const subscription of doc['x-theuth'].subscriptions) {
  const method = methods.find(m => m.name === subscription.subscribe)
  const owner = method['x-theuth-owner'].replace(/[^a-zA-Z0-9]/g, c => `_${c.charCodeAt(0).toString(16)}`)
  add(`method.${owner}.output`, `${identifier(subscription.subscribe)}Item`, subscription.itemSchema)
  add(`method.${owner}.receiver`, `${identifier(subscription.subscribe)}Item`, subscription.receiverSchema)
}
function rewrite(schema, group) {
  if (schema === true) return { tsType: 'any' }
  if (schema === false) return false
  const result = { ...schema }
  if (schema.$ref) {
    const target = schema.$ref.replace('#/components/schemas/', '')
    if (!(target in schemas) || scope(target) !== group) throw new Error(`Invalid or cross-domain ref: ${schema.$ref}`)
    result.$ref = `#/definitions/${name(target)}`
  }
  for (const key of ['properties', 'patternProperties', 'definitions', '$defs']) {
    if (schema[key]) result[key] = Object.fromEntries(Object.entries(schema[key]).map(([k, v]) => [k, rewrite(v, group)]))
  }
  for (const key of ['allOf', 'anyOf', 'oneOf']) {
    if (schema[key]) result[key] = schema[key].map(v => rewrite(v, group))
  }
  for (const key of ['additionalProperties', 'additionalItems', 'not', 'contains', 'if', 'then', 'else']) {
    if (schema[key] !== undefined) result[key] = rewrite(schema[key], group)
  }
  if (schema.items !== undefined) result.items = Array.isArray(schema.items) ? schema.items.map(v => rewrite(v, group)) : rewrite(schema.items, group)
  // A description/default-only Value schema accepts every JSON value.
  if (!['type', '$ref', 'properties', 'allOf', 'anyOf', 'oneOf', 'const', 'enum', 'not'].some(k => k in schema)) result.tsType = 'any'
  // The compiler otherwise drops sibling properties beside combiners (Advisory,
  // MeetingEvent::Detected). Draft-7 means intersection, not replacement.
  if (result.properties && ['allOf', 'anyOf', 'oneOf'].some(k => result[k])) {
    const { allOf = [], anyOf, oneOf, ...object } = result
    return { allOf: [object, ...allOf, ...(anyOf ? [{ anyOf }] : []), ...(oneOf ? [{ oneOf }] : [])] }
  }
  return result
}
await rm(out, { recursive: true, force: true })
await mkdir(out, { recursive: true })
for (const [group, definitions] of [...groups].sort(([a], [b]) => a.localeCompare(b))) {
  const defs = Object.fromEntries(Object.entries(definitions).map(([key, schema]) => [key, { ...(schema === false ? { not: {} } : rewrite(schema, group)), title: key }]))
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
  return [...(tag?.const ? [tag.const] : tag?.enum ?? []), ...(schema.oneOf ?? schema.anyOf ?? schema.allOf ?? []).flatMap(variants)]
}
// JSON-RPC permits omitted trailing arguments. Optional tuple slots also accept
// explicit undefined (JSON.stringify encodes that positional slot as null).
const signatures = methodTypes.map(([method, args, result]) => {
  const parameters = methods.find(m => m.name === method).params
  const required = parameters.findLastIndex(p => p.required) + 1
  const tuple = `[${parameters.map((_, i) => `${type(args)}[${i}]${i >= required ? '?' : ''}`).join(', ')}]`
  return `  ${JSON.stringify(method)}: { params: ${tuple}; result: ${type(result)} }`
}).join('\n')
const topics = doc['x-theuth'].events.map(e => `  ${JSON.stringify(`${e.topic}@${e.version}`)}: ${schemaType(e.schema)}`).join('\n')
const receiverTopics = doc['x-theuth'].events.map(e => `  ${JSON.stringify(`${e.topic}@${e.version}`)}: ${schemaType(e.receiverSchema)}`).join('\n')
const receiverResults = methodTypes.map(([method, , , receiver]) => `  ${JSON.stringify(method)}: ${type(receiver)}`).join('\n')
const metadata = Object.fromEntries(doc['x-theuth'].events.map(e => [`${e.topic}@${e.version}`, variants(e.schema)]))
const taggedTopics = doc['x-theuth'].events.filter(e => variants(e.schema).length).map(e => `  ${JSON.stringify(`${e.topic}@${e.version}`)}: ${schemaType(e.schema)}`).join('\n')
const subscriptions = Object.fromEntries(doc['x-theuth'].subscriptions.map(s => [s.subscribe, { unsubscribe: s.unsubscribe, notification: s.notification }]))
const items = doc['x-theuth'].subscriptions.map(s => {
  const method = methods.find(m => m.name === s.subscribe)
  const owner = method['x-theuth-owner'].replace(/[^a-zA-Z0-9]/g, c => `_${c.charCodeAt(0).toString(16)}`)
  return `  ${JSON.stringify(s.subscribe)}: ${type(`method.${owner}.output.${identifier(s.subscribe)}Item`)}`
}).join('\n')
const receiverItems = doc['x-theuth'].subscriptions.map(s => {
  const method = methods.find(m => m.name === s.subscribe)
  const owner = method['x-theuth-owner'].replace(/[^a-zA-Z0-9]/g, c => `_${c.charCodeAt(0).toString(16)}`)
  return `  ${JSON.stringify(s.subscribe)}: ${type(`method.${owner}.receiver.${identifier(s.subscribe)}Item`)}`
}).join('\n')
const componentKeys = Object.keys(schemas).filter(key => key.includes('.output.') && schemas[key.replace('.output.', '.receiver.')])
const componentResults = componentKeys.map(key => `  ${JSON.stringify(key)}: ${type(key)}`).join('\n')
const componentReceivers = componentKeys.map(key => `  ${JSON.stringify(key)}: ${type(key.replace('.output.', '.receiver.'))}`).join('\n')
const body = `import { receiveResult } from './receive'\nexport interface MethodMap {\n${signatures}\n}\nexport type Api = { [M in keyof MethodMap]: (...args: MethodMap[M]['params']) => Promise<MethodMap[M]['result']> }\nexport const METHODS = ${JSON.stringify(methodTypes.map(([m]) => m))} as const\nexport function makeApi(client: { call<T>(method: string, params: unknown[]): Promise<T> }): Api {\n  return Object.fromEntries(METHODS.map(method => [method, (...args: unknown[]) => client.call(method, args).then(result => receiveResult(method, result))])) as unknown as Api\n}\nexport interface TopicPayloads {\n${topics}\n}\nexport interface TaggedTopicPayloads {\n${taggedTopics}\n}\nexport type TopicKey = keyof TopicPayloads\nexport type TopicEvent = { [K in TopicKey]: { key: K; event: TopicPayloads[K] } }[TopicKey]\nexport const TOPICS = ${JSON.stringify(metadata)} as const\nexport const SUBSCRIPTIONS = ${JSON.stringify(subscriptions)} as const\nexport interface SubscriptionItems {\n${items}\n}\nexport interface SubscriptionReceiverItems {\n${receiverItems}\n}\n`
await writeFile(`${out}/client.ts`, `/* Generated from OpenRPC; do not edit. */\n${[...imports].map(([group, alias]) => `import type * as ${alias} from './${group}'`).join('\n')}\n${body}\nexport interface ReceiverResults {\n${receiverResults}\n}\nexport interface TopicReceiverPayloads {\n${receiverTopics}\n}\nexport interface ComponentResults {\n${componentResults}\n}\nexport interface ComponentReceivers {\n${componentReceivers}\n}\n`)
await writeFile(`${out}/receive.ts`, emitAdapters(doc, schemas))
await writeFile(`${out}/components.json`, JSON.stringify(Object.fromEntries([...groups].flatMap(([group, defs]) => Object.keys(defs).map(n => [`${group}.${n}`, { path: group, name: toSafeString(n) }]))), null, 2))
console.log(`Generated ${methods.length} methods, ${doc['x-theuth'].events.length} topics, ${groups.size} scoped modules from ${input}`)
