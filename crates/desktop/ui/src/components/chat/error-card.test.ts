import { createElement } from 'react'
import { renderToStaticMarkup } from 'react-dom/server'
import { expect, test } from 'vitest'
import { TooltipProvider } from '@/components/ui/tooltip'
import { useToolRuns } from '@/lib/tool-runs'
import { DisclosureScope } from './disclosure'
import { ErrorCard } from './ErrorCard'
import { parseError } from './error-card'

const GEMINI_503 = `model: Web stream error for model 'gemini::gemini-flash-latest (adapter: Gemini)'.
Cause: HTTP error.
Status: 503 Service Unavailable Service Unavailable
Body: {
  "error": {
    "code": 503,
    "message": "This model is currently experiencing high demand. Spikes in demand are usually temporary. Please try again later.",
    "status": "UNAVAILABLE"
  }
}`

const LITELLM_400 = `Web stream error for model 'zai-org/GLM-5.3-Flash': status 400. Body: {"error":{"message":"litellm.BadRequestError: OpenAIException - max_tokens is too large","type":"invalid_request_error","code":400}}`

const CHILD_400 = `↳ probe-gemini error: model: Web stream error for model 'gemini-flash-latest (adapter: OpenAI)'.
Cause: HTTP error.
Status: 400 Bad Request Bad Request
Body: [{
  "error": {
    "code": 400,
    "message": "Invalid JSON payload received. Unknown name \\"prompt_cache_key\\": Cannot find field.\\nInvalid JSON payload received. Unknown name \\"prompt_cache_retention\\": Cannot find field.",
    "status": "INVALID_ARGUMENT",
    "details": [ { "@type": "type.googleapis.com/google.rpc.BadRequest", "fieldViolations": [ { "description": "Invalid JSON payload received. Unknown name \\"prompt_cache_key\\": Cannot find field." }, { "description": "Invalid JSON payload received. Unknown name \\"prompt_cache_retention\\": Cannot find field." } ] } ]
  }
}]`

test('the Gemini 503: overloaded, the body message, model / provider / status, a retry hint', () => {
  const e = parseError(GEMINI_503)
  expect(e).toMatchObject({
    kind: 'overloaded',
    title: 'Gemini is overloaded',
    source: 'model',
    model: 'gemini-flash-latest',
    provider: 'gemini',
    adapter: 'Gemini',
    status: 503,
    reason: 'Service Unavailable',
    cause: 'HTTP error',
    errorType: 'UNAVAILABLE',
    message: 'This model is currently experiencing high demand. Spikes in demand are usually temporary. Please try again later.',
    compact: false,
  })
  expect(e.hint).toMatch(/temporary/)
  expect(e.prettyBody).toContain('"status": "UNAVAILABLE"')
})

test('an inline `status 400. Body: {…}` (litellm): bad request with the litellm message and type', () => {
  expect(parseError(LITELLM_400)).toMatchObject({
    kind: 'bad_request',
    title: 'Bad request',
    model: 'zai-org/GLM-5.3-Flash',
    status: 400,
    reason: 'Bad Request',
    errorType: 'invalid_request_error',
    message: 'litellm.BadRequestError: OpenAIException - max_tokens is too large',
  })
})

test('a relayed child error: the `↳ name error:` prefix, a JSON array body, escaped quotes and newlines', () => {
  const e = parseError(CHILD_400)
  expect(e).toMatchObject({
    kind: 'bad_request',
    title: 'Bad request',
    child: 'probe-gemini',
    source: 'model',
    model: 'gemini-flash-latest',
    adapter: 'OpenAI',
    status: 400,
    errorType: 'INVALID_ARGUMENT',
    message: 'Invalid JSON payload received. Unknown name "prompt_cache_key": Cannot find field.\nInvalid JSON payload received. Unknown name "prompt_cache_retention": Cannot find field.',
  })
  expect(e.provider).toBeUndefined()
  expect(e.hint).toBe("The provider rejected the request's parameters.")
})

test('a 429 is rate limited; a 401 is an auth failure', () => {
  const rl = parseError(`model: Web stream error for model 'openai::gpt-5'.\nStatus: 429 Too Many Requests\nBody: {"error":{"message":"Rate limit reached for requests","type":"requests","code":"rate_limit_exceeded"}}`)
  expect(rl).toMatchObject({ kind: 'rate_limited', title: 'Rate limited by OpenAI', status: 429, message: 'Rate limit reached for requests' })
  const auth = parseError(`model: Web stream error for model 'anthropic::claude-x': status 401. Body: {"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}`)
  expect(auth).toMatchObject({ kind: 'auth', title: 'Anthropic rejected the credentials', status: 401, errorType: 'authentication_error', message: 'invalid x-api-key' })
})

test('a plain one-line error is compact: the text is the title, nothing else', () => {
  const e = parseError('no API key configured for provider foo')
  expect(e).toMatchObject({ kind: 'unknown', title: 'no API key configured for provider foo', compact: true })
  expect(e.message).toBeUndefined()
  expect(e.model).toBeUndefined()
  expect(e.status).toBeUndefined()
  expect(e.hint).toBeUndefined()
  expect(parseError('model: connection refused')).toMatchObject({ kind: 'network', compact: true, source: 'model', title: 'connection refused' })
})

test('a truncated / malformed JSON body still yields its message, type and code', () => {
  const e = parseError(`model: Web stream error for model 'gemini::gemini-pro'.\nStatus: 500 Internal Server Error\nBody: {"error": {"code": 500, "status": "INTERNAL", "message": "An internal error has occurred. Please \\"retry\\" or rep`)
  expect(e).toMatchObject({ kind: 'server', title: 'Gemini server error', status: 500, errorType: 'INTERNAL' })
  expect(e.message).toBe('An internal error has occurred. Please "retry" or rep')
  expect(e.prettyBody).toBeUndefined()
  expect(parseError('upstream said: {"error": oops}').compact).toBe(false)
})

test('a message that is stringified JSON in turn yields the inner message', () => {
  const inner = JSON.stringify({ error: { message: 'context_length_exceeded: prompt is too long', type: 'invalid_request_error' } })
  const e = parseError(`model: status 400. Body: ${JSON.stringify({ error: { message: inner, code: 400 } })}`)
  expect(e).toMatchObject({ kind: 'context_too_long', title: 'Context too long', message: 'context_length_exceeded: prompt is too long' })
})

const render = (text: string, child?: string) => renderToStaticMarkup(createElement(TooltipProvider, null, createElement(DisclosureScope, { value: 'e' }, createElement(ErrorCard, { text, child }))))

test('the card: title, message, chips, hint; the full text only once the fold opens', () => {
  useToolRuns.setState({ disclosed: {} })
  const closed = render(GEMINI_503)
  expect(closed).toContain('role="alert"')
  expect(closed).toContain('Gemini is overloaded')
  expect(closed).toContain('experiencing high demand')
  expect(closed).toContain('gemini-flash-latest')
  expect(closed).toContain('503 UNAVAILABLE')
  expect(closed).toContain('Usually temporary')
  expect(closed).toContain('Full error')
  expect(closed).not.toContain('Cause: HTTP error')
  useToolRuns.setState({ disclosed: { 'e|error': true } })
  const open = render(GEMINI_503)
  expect(open).toContain('Cause: HTTP error')
  expect(open).toContain('Copy error')
})

test('the card: a child attribution, two message lines then +N more; a compact error has no fold', () => {
  useToolRuns.setState({ disclosed: {} })
  const html = render(CHILD_400, 'probe-gemini')
  expect(html).toContain('↳ probe-gemini')
  expect(html).toContain('400 INVALID_ARGUMENT')
  expect(html).toContain('OpenAI')
  const three = render(`model: status 400. Body: {"error":{"message":"one\\ntwo\\nthree"}}`)
  expect(three).toContain('+1 more')
  const compact = render('send failed')
  expect(compact).toContain('send failed')
  expect(compact).not.toContain('Full error')
})
