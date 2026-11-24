/**
 * Best-effort structure of an `error` row's text — what the error card shows instead of the raw dump. Parsed on the
 * client (the engine's strings are free-form: `model: Web stream error for model '…'.\nStatus: 503 …\nBody: {…}`), so
 * transcripts saved before the card read the same. Every field is optional; text with nothing to parse still yields a
 * title (`compact`: the card is the text alone).
 */

export type ErrorKind = 'overloaded' | 'rate_limited' | 'auth' | 'context_too_long' | 'bad_request' | 'server' | 'timeout' | 'network' | 'unknown'

export interface ParsedError {
  kind: ErrorKind
  /** The headline: `Gemini is overloaded`, `Bad request`; a compact error's own text. */
  title: string
  /** The human message (from the JSON body when there is one); may span lines. Absent when it would repeat the title. */
  message?: string
  /** One line on what to do, for the kinds that have one. */
  hint?: string
  /** The subagent a parent relays the error from (`↳ name error: …`). */
  child?: string
  /** The leading `model:` style tag the engine puts on the text. */
  source?: string
  /** The model id without its `provider::` namespace or `(adapter: …)` suffix. */
  model?: string
  /** The `provider::` namespace of the model id. */
  provider?: string
  /** The `(adapter: X)` the engine names. */
  adapter?: string
  status?: number
  reason?: string
  cause?: string
  /** The body's error type / status: `UNAVAILABLE`, `invalid_request_error`. */
  errorType?: string
  /** The raw body text, and the body pretty-printed when it parsed as JSON. */
  body?: string
  prettyBody?: string
  /** Nothing parsed and one line: the card shows the text as its title, with no details fold. */
  compact: boolean
  /** The text the card's details show verbatim. */
  text: string
}

const PROVIDERS: Record<string, string> = {
  openai: 'OpenAI',
  anthropic: 'Anthropic',
  gemini: 'Gemini',
  google: 'Google',
  vertex: 'Vertex AI',
  xai: 'xAI',
  deepseek: 'DeepSeek',
  openrouter: 'OpenRouter',
  ollama: 'Ollama',
  groq: 'Groq',
  mistral: 'Mistral',
  cohere: 'Cohere',
  together: 'Together',
  fireworks: 'Fireworks',
  zai: 'Z.ai',
  bedrock: 'Bedrock',
  azure: 'Azure',
}

/** `gemini` → `Gemini`, `openai` → `OpenAI`; an unknown name keeps its spelling, first letter raised. */
export function providerName(id: string): string {
  return PROVIDERS[id.toLowerCase()] ?? id.charAt(0).toUpperCase() + id.slice(1)
}

const REASONS: Record<number, string> = {
  400: 'Bad Request',
  401: 'Unauthorized',
  403: 'Forbidden',
  404: 'Not Found',
  408: 'Request Timeout',
  409: 'Conflict',
  413: 'Payload Too Large',
  422: 'Unprocessable Entity',
  429: 'Too Many Requests',
  500: 'Internal Server Error',
  502: 'Bad Gateway',
  503: 'Service Unavailable',
  504: 'Gateway Timeout',
  529: 'Overloaded',
}

const HINTS: Partial<Record<ErrorKind, string>> = {
  overloaded: 'Usually temporary — try again in a moment, or switch models.',
  rate_limited: 'Too many requests — wait a little before retrying, or check the quota.',
  auth: 'Check the API key or credentials for this provider in Settings.',
  context_too_long: 'Too long for this model — compact the conversation or pick a model with a larger context.',
  bad_request: "The provider rejected the request's parameters.",
  server: 'The provider had an internal error — try again.',
  timeout: 'The provider took too long to answer — try again.',
  network: "Couldn't reach the provider — check the connection.",
}

/** `Service Unavailable Service Unavailable` → `Service Unavailable` (reqwest prints the reason twice). */
function dedupeReason(reason: string): string {
  const words = reason.trim().split(/\s+/)
  if (words.length % 2 === 0) {
    const half = words.length / 2
    const a = words.slice(0, half).join(' ')
    if (a === words.slice(half).join(' ')) return a
  }
  return words.join(' ')
}

/** The balanced JSON value starting at `text[from]` (`{` or `[`), string-aware; undefined when it never closes. */
function balanced(text: string, from: number): string | undefined {
  let depth = 0
  let inString = false
  for (let i = from; i < text.length; i++) {
    const c = text[i]
    if (inString) {
      if (c === '\\') i++
      else if (c === '"') inString = false
    } else if (c === '"') inString = true
    else if (c === '{' || c === '[') depth++
    else if (c === '}' || c === ']') {
      depth--
      if (depth === 0) return text.slice(from, i + 1)
    }
  }
  return undefined
}

function tryJson(text: string): unknown {
  try {
    return JSON.parse(text)
  } catch {
    return undefined
  }
}

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v)
const str = (v: unknown): string | undefined => (typeof v === 'string' && v.trim() ? v.trim() : undefined)

interface BodyFacts {
  message?: string
  errorType?: string
  code?: number
}

/** The message, type and code of a parsed body: `{error:{message,status|type,code}}`, `{message}`, `{detail}`, an array
 * of those (Gemini), a message that is itself JSON. */
function bodyFacts(value: unknown, depth = 0): BodyFacts {
  if (depth > 4) return {}
  if (Array.isArray(value)) {
    for (const item of value) {
      const facts = bodyFacts(item, depth + 1)
      if (facts.message || facts.errorType) return facts
    }
    return {}
  }
  if (!isObject(value)) return {}
  const err = isObject(value.error) ? value.error : value
  let message = str(err.message) ?? str(value.message) ?? str(err.detail) ?? str(value.detail) ?? str(value.error) ?? str(err.error_description)
  if (!message) {
    const detail = Array.isArray(err.detail) ? err.detail : Array.isArray(value.detail) ? value.detail : undefined
    const msgs = detail?.map((d) => (isObject(d) ? str(d.msg) ?? str(d.message) : str(d))).filter((m): m is string => !!m)
    if (msgs?.length) message = msgs.join('\n')
  }
  const errorType = str(err.status) ?? str(err.type) ?? str(value.type) ?? (typeof err.code === 'string' ? str(err.code) : undefined)
  const rawCode = typeof err.code === 'number' ? err.code : typeof value.code === 'number' ? value.code : typeof value.status === 'number' ? value.status : undefined
  const code = rawCode !== undefined && rawCode >= 100 && rawCode < 600 ? rawCode : undefined
  // A message that is JSON in turn (a proxy wrapping the upstream body): its own message wins.
  if (message && /^[[{]/.test(message)) {
    const inner = tryJson(message) ?? (() => {
      const b = balanced(message, 0)
      return b ? tryJson(b) : undefined
    })()
    if (inner !== undefined) {
      const nested = bodyFacts(inner, depth + 1)
      if (nested.message) return { message: nested.message, errorType: nested.errorType ?? errorType, code: nested.code ?? code }
    }
  }
  return { message, errorType, code }
}

/** A truncated or malformed body: pull `"message": "…"` (even unterminated) and friends out by pattern. */
function scrapeFacts(body: string): BodyFacts {
  const m = /"(?:message|detail|error_description)"\s*:\s*"((?:[^"\\]|\\.)*)/.exec(body)
  let message: string | undefined
  if (m) {
    const raw = (m[1] ?? '').replace(/\\$/, '')
    message = str((tryJson(`"${raw}"`) as string | undefined) ?? raw.replace(/\\n/g, '\n').replace(/\\"/g, '"').replace(/\\\\/g, '\\'))
  }
  const t = /"(?:status|type)"\s*:\s*"([A-Za-z_.]+)"/.exec(body)
  const c = /"code"\s*:\s*(\d{3})\b/.exec(body)
  return { message, errorType: t?.[1], code: c ? Number(c[1]) : undefined }
}

function classify(status: number | undefined, haystack: string): ErrorKind {
  if (status === 429) return 'rate_limited'
  if (status === 503 || status === 529) return 'overloaded'
  if (status === 401 || status === 403) return 'auth'
  if (status === 413) return 'context_too_long'
  if (status === 408 || status === 504) return 'timeout'
  if (/context.{0,24}(length|window|too long|limit)|maximum context|too many tokens|prompt is too long|context_length_exceeded/i.test(haystack)) return 'context_too_long'
  if (status === 400 || status === 404 || status === 409 || status === 422) return 'bad_request'
  if (status !== undefined && status >= 500) return 'server'
  if (/rate.?limit|too many requests|RESOURCE_EXHAUSTED|quota/i.test(haystack)) return 'rate_limited'
  if (/overloaded|high demand|\bUNAVAILABLE\b|over capacity/i.test(haystack)) return 'overloaded'
  if (/unauthori[sz]ed|invalid.{0,6}api.?key|PERMISSION_DENIED|UNAUTHENTICATED|authentication|forbidden/i.test(haystack)) return 'auth'
  if (/invalid_request_error|INVALID_ARGUMENT|bad request/i.test(haystack)) return 'bad_request'
  if (/timed? ?out|timeout|deadline exceeded/i.test(haystack)) return 'timeout'
  if (/connection (refused|reset|closed|aborted)|network|dns|failed to connect|error sending request|unreachable|tls handshake/i.test(haystack)) return 'network'
  return 'unknown'
}

function titleFor(kind: ErrorKind, who: string | undefined, source: string | undefined, status: number | undefined): string {
  switch (kind) {
    case 'overloaded':
      return `${who ?? 'The model'} is overloaded`
    case 'rate_limited':
      return who ? `Rate limited by ${who}` : 'Rate limited'
    case 'auth':
      return who ? `${who} rejected the credentials` : 'Authentication failed'
    case 'context_too_long':
      return 'Context too long'
    case 'bad_request':
      return status === 404 ? 'Not found' : 'Bad request'
    case 'server':
      return who ? `${who} server error` : 'Server error'
    case 'timeout':
      return 'Request timed out'
    case 'network':
      return 'Network error'
    case 'unknown':
      return source === 'model' ? 'Model error' : 'Error'
  }
}

export function parseError(raw: string): ParsedError {
  let rest = raw.trim()
  let child: string | undefined
  const relay = /^↳\s*(\S+)\s+error:\s*/.exec(rest)
  if (relay) {
    child = relay[1]
    rest = rest.slice(relay[0].length)
  }
  let source: string | undefined
  const tag = /^([a-z][\w-]{0,23}):\s+(?=\S)/.exec(rest)
  if (tag) {
    source = tag[1]
    rest = rest.slice(tag[0].length)
  }

  // Model id: `for model 'gemini::x (adapter: Gemini)'`.
  let model: string | undefined
  let provider: string | undefined
  let adapter: string | undefined
  const mm = /\bmodel\s+['`"]([^'`"]+)['`"]/.exec(rest)
  if (mm) {
    let id = (mm[1] ?? '').trim()
    const ad = /\s*\(adapter:\s*([^)]+)\)\s*$/.exec(id)
    if (ad) {
      adapter = (ad[1] ?? '').trim() || undefined
      id = id.slice(0, ad.index).trim()
    }
    const ns = /^([\w-]+)::(.+)$/.exec(id)
    if (ns) {
      provider = ns[1]
      id = ns[2] ?? id
    }
    model = id || undefined
  }

  // Body: after `Body:`, else the first JSON-looking value.
  let body: string | undefined
  let parsed: unknown
  const bodyAt = /\bBody:\s*/.exec(rest)
  const jsonAt = bodyAt ? bodyAt.index + bodyAt[0].length : rest.search(/[{[]\s*["{[]/)
  if (jsonAt >= 0 && /[{[]/.test(rest[jsonAt] ?? '')) {
    const span = balanced(rest, jsonAt)
    body = (span ?? rest.slice(jsonAt)).trim()
    parsed = span ? tryJson(span) : undefined
  } else if (bodyAt) {
    body = rest.slice(jsonAt).trim() || undefined
  }
  const facts: BodyFacts = parsed !== undefined ? bodyFacts(parsed) : body ? scrapeFacts(body) : {}
  const prettyBody = parsed !== undefined ? JSON.stringify(parsed, null, 2) : undefined

  // Status: `Status: 503 Service Unavailable`, `status 400`, `HTTP 429`, else the body's code.
  let status: number | undefined
  let reason: string | undefined
  const sl = /^\s*Status:\s*(\d{3})\b[ \t]*(.*)$/m.exec(rest)
  const inline = sl ?? /\bstatus(?:\s+code)?[:=\s]+(\d{3})\b/i.exec(rest) ?? /\bHTTP(?:\s+error)?[:\s]+(\d{3})\b/i.exec(rest)
  if (inline) {
    status = Number(inline[1])
    if (sl?.[2]?.trim()) reason = dedupeReason(sl[2])
  } else if (facts.code !== undefined) status = facts.code
  if (status !== undefined && !reason) reason = REASONS[status]

  const cause = str(/^\s*Cause:\s*(.+?)\.?\s*$/m.exec(rest)?.[1])

  const lines = rest.split('\n').filter((l) => l.trim())
  const compact = !model && status === undefined && body === undefined && lines.length <= 1
  const kind = classify(status, `${facts.errorType ?? ''} ${facts.message ?? ''} ${rest}`)
  const hint = HINTS[kind]
  const base = { child, source, model, provider, adapter, status, reason, cause, errorType: facts.errorType, body, prettyBody, text: raw.trim() }

  if (compact) return { ...base, kind, title: rest || raw.trim() || 'Error', hint: kind === 'unknown' ? undefined : hint, compact }

  const who = provider ? providerName(provider) : adapter
  const title = titleFor(kind, who, source, status)
  // The message: the body's, else the first line that is not the engine's framing.
  let message = facts.message
  if (!message) {
    const head = lines.find((l) => !/^\s*(Cause|Status|Body):/.test(l))?.trim()
    message = head && !/^Web stream error for model/i.test(head) ? head : cause && cause !== 'HTTP error' ? cause : head
  }
  if (message === title) message = undefined
  return { ...base, kind, title, message, hint, compact }
}
