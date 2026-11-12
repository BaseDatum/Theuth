import { z } from 'zod'
import { TOPICS, type TopicEvent, type TopicKey } from '@/rpc/bindings/client'
import { receiveTopic } from '@/rpc/bindings/receive'

const envelope = z.object({ topic: z.string(), version: z.number().int().positive(), event: z.unknown() }).passthrough()
const payload = z.record(z.string(), z.unknown())
const knownTopics: Readonly<Record<string, readonly string[]>> = TOPICS
// Only unambiguous old tags can identify a topic. Collisions stay deliberately unrouted.
const legacy: Record<string, TopicKey | null> = Object.create(null)
for (const [key, variants] of Object.entries(TOPICS)) {
  for (const type of variants) legacy[type] = Object.hasOwn(legacy, type) ? null : key as TopicKey
}

export function readTopic(value: unknown, malformed: () => void): TopicEvent | undefined {
  const parsed = envelope.safeParse(value)
  if (!parsed.success) { malformed(); return }
  const { topic, version, event } = parsed.data
  const key = `${topic}@${version}`
  if (!Object.hasOwn(knownTopics, key)) return
  const body = payload.safeParse(event)
  if (!body.success) { malformed(); return }
  const variants = knownTopics[key]!
  if (variants.length) {
    if (typeof body.data.type !== 'string') { malformed(); return }
    if (!variants.includes(body.data.type)) return
  }
  // Only routing discriminants are validated; owner payload validation stays on the server.
  const routed = { key, event: receiveTopic(key, body.data) } as TopicEvent
  return routed
}
export function readLegacy(value: unknown, malformed: () => void): TopicEvent | undefined {
  const parsed = payload.safeParse(value)
  if (!parsed.success || typeof parsed.data.type !== 'string') { malformed(); return }
  const key = Object.hasOwn(legacy, parsed.data.type) ? legacy[parsed.data.type] : undefined
  if (!key) return
  const at = key.lastIndexOf('@')
  return readTopic({ topic: key.slice(0, at), version: Number(key.slice(at + 1)), event: value }, malformed)
}
