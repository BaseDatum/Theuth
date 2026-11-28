import { parseExpression, split, tokenize } from '@openuidev/lang-core'
import { type Binding, check, type Diagnostic, type ToolSnapshot } from '@theuth/apps'
import type { AppRendererProps } from '@theuth/apps/renderer'
import type { InvokeActionOptions } from '@/components/chat/surface/deck-context'
import { lookupPath } from './DeckSurfaceSection'
import type { DeckRuntimeStatus, InteractResponse, ScriptRevision } from './types'

/** Mirror of `document::deck::MAX_OPENUI_CONTENT_BYTES` (64 KiB). */
export const MAX_OPENUI_SOURCE_BYTES = 64 * 1024

export const DECK_DATA_TOOL = 'deck_data'
export const DECK_STATUS_TOOL = 'deck_status'

const SYNTHETIC_LOCAL_TOOLS = new Set<string>([DECK_DATA_TOOL, DECK_STATUS_TOOL])
const FORBIDDEN_KEYS = new Set(['__proto__', 'constructor', 'prototype'])

const DECK_DATA_SNAPSHOT: ToolSnapshot = {
  name: DECK_DATA_TOOL,
  risk: 'read_only',
  identity: 'deck-local:deck_data:v1',
  schema: {
    type: 'object',
    properties: {
      path: { type: 'string' },
    },
    additionalProperties: false,
  },
}

const DECK_STATUS_SNAPSHOT: ToolSnapshot = {
  name: DECK_STATUS_TOOL,
  risk: 'read_only',
  identity: 'deck-local:deck_status:v1',
  schema: {
    type: 'object',
    properties: {},
    additionalProperties: false,
  },
}

/**
 * Build the ONLY OpenUI tool catalog visible to a Deck OpenUI section:
 * - Synthetic local read-only tools: `deck_data` and `deck_status`
 * - Declared actions of the Deck's currently APPROVED `activeScript` (never pending/rejected)
 *
 * Session tools and MCP tools are NEVER exposed here.
 */
export function buildDeckOpenUITools(
  deckId: string,
  activeScript: ScriptRevision | null | undefined,
): ToolSnapshot[] {
  const tools: ToolSnapshot[] = [DECK_DATA_SNAPSHOT, DECK_STATUS_SNAPSHOT]
  if (!activeScript || activeScript.status !== 'approved') {
    return tools
  }
  const seen = new Set<string>(SYNTHETIC_LOCAL_TOOLS)
  for (const act of activeScript.declared_actions ?? []) {
    const name = typeof act.name === 'string' ? act.name.trim() : ''
    if (!name || seen.has(name)) continue
    seen.add(name)
    tools.push({
      name,
      risk: act.mutates_state ? 'mutating' : 'read_only',
      identity: `deck-action:${deckId}:rev${activeScript.rev}:${name}`,
      schema: act.schema && typeof act.schema === 'object' ? act.schema : { type: 'object' },
    })
  }
  return tools
}

export type DeckOpenUIAnalysis = {
  ok: boolean
  source: string
  tools: ToolSnapshot[]
  bindings: Binding[]
  /** `Query` statement IDs bound ONLY to local `deck_data` or `deck_status`. */
  localQueryIds: string[]
  /** `Query` statement IDs bound to read-only declared Monty actions. */
  actionQueryIds: string[]
  diagnostics: Diagnostic[]
}

function utf8ByteLength(text: string): number {
  return new TextEncoder().encode(text).byteLength
}

/**
 * Fail-closed static analysis for a Deck OpenUI section:
 * - Enforces <= 64 KiB UTF-8 source cap
 * - Runs `@theuth/apps/check` against the synthetic Deck-only tool catalog
 * - Rejects `Query` targeting any mutating declared action
 * - Rejects polling (`refreshInterval`) on any Monty action `Query`
 * - Rejects `Mutation` targeting local read-only `deck_data` / `deck_status`
 */
export function analyzeDeckOpenUISource(
  source: string,
  deckId: string,
  activeScript: ScriptRevision | null | undefined,
): DeckOpenUIAnalysis {
  const tools = buildDeckOpenUITools(deckId, activeScript)
  const trimmed = typeof source === 'string' ? source.trim() : ''

  if (!trimmed) {
    return {
      ok: false,
      source: '',
      tools,
      bindings: [],
      localQueryIds: [],
      actionQueryIds: [],
      diagnostics: [{ at: '', code: 'no-root', message: 'OpenUI section source is empty.' }],
    }
  }

  const bytes = utf8ByteLength(source)
  if (bytes > MAX_OPENUI_SOURCE_BYTES) {
    return {
      ok: false,
      source: '',
      tools,
      bindings: [],
      localQueryIds: [],
      actionQueryIds: [],
      diagnostics: [
        {
          at: '',
          code: 'budget-exceeded',
          message: `OpenUI section source exceeds ${MAX_OPENUI_SOURCE_BYTES} bytes (got ${bytes}).`,
        },
      ],
    }
  }

  const checked = check(trimmed, tools)
  const diagnostics: Diagnostic[] = [...checked.diagnostics]
  const toolsByName = new Map(tools.map((t) => [t.name, t]))

  // Enforce Deck-specific Query/Mutation AST policies (no mutating queries, no polling on Monty
  // action queries, no mutations targeting synthetic local tools).
  try {
    const rawStatements = split(tokenize(checked.source))
    for (const raw of rawStatements) {
      let expr
      try {
        expr = parseExpression(raw.tokens)
      } catch {
        continue
      }
      if (!expr || expr.k !== 'Comp') continue
      if (expr.name === 'Query') {
        const [toolAst, , , refreshAst] = expr.args
        if (toolAst && toolAst.k === 'Str') {
          const toolName = toolAst.v
          const toolSnap = toolsByName.get(toolName)
          if (toolSnap && toolSnap.risk !== 'read_only') {
            diagnostics.push({
              at: raw.id,
              code: 'tool-args',
              message: `Query cannot invoke mutating Deck action "${toolName}"; declare a Mutation instead.`,
            })
          }
          if (
            toolSnap &&
            !SYNTHETIC_LOCAL_TOOLS.has(toolName) &&
            refreshAst &&
            refreshAst.k !== 'Null'
          ) {
            diagnostics.push({
              at: raw.id,
              code: 'tool-args',
              message: `Polling refresh interval is not permitted on Monty action query "${toolName}".`,
            })
          }
        }
      } else if (expr.name === 'Mutation') {
        const [toolAst] = expr.args
        if (toolAst && toolAst.k === 'Str' && SYNTHETIC_LOCAL_TOOLS.has(toolAst.v)) {
          diagnostics.push({
            at: raw.id,
            code: 'tool-args',
            message: `Mutation cannot target local read-only tool "${toolAst.v}".`,
          })
        }
      }
    }
  } catch (err) {
    diagnostics.push({
      at: '',
      code: 'parse-exception',
      message: err instanceof Error ? err.message : 'Failed to inspect OpenUI statements.',
    })
  }

  // Defense-in-depth binding check even if statement tokens differed.
  for (const b of checked.bindings) {
    const toolSnap = toolsByName.get(b.tool)
    if (!toolSnap) {
      diagnostics.push({
        at: b.statement_id,
        code: 'tool-unknown',
        message: `Tool "${b.tool}" is not in the Deck catalog.`,
      })
      continue
    }
    if (b.kind === 'query' && toolSnap.risk !== 'read_only') {
      if (!diagnostics.some((d) => d.at === b.statement_id && d.code === 'tool-args')) {
        diagnostics.push({
          at: b.statement_id,
          code: 'tool-args',
          message: `Query cannot invoke mutating Deck action "${b.tool}".`,
        })
      }
    }
    if (b.kind === 'mutation' && SYNTHETIC_LOCAL_TOOLS.has(b.tool)) {
      if (!diagnostics.some((d) => d.at === b.statement_id && d.code === 'tool-args')) {
        diagnostics.push({
          at: b.statement_id,
          code: 'tool-args',
          message: `Mutation cannot target local read-only tool "${b.tool}".`,
        })
      }
    }
  }

  const localQueryIds = checked.bindings
    .filter((b) => b.kind === 'query' && SYNTHETIC_LOCAL_TOOLS.has(b.tool))
    .map((b) => b.statement_id)
  const actionQueryIds = checked.bindings
    .filter((b) => b.kind === 'query' && !SYNTHETIC_LOCAL_TOOLS.has(b.tool))
    .map((b) => b.statement_id)

  return {
    ok: diagnostics.length === 0,
    source: checked.source,
    tools,
    bindings: checked.bindings,
    localQueryIds,
    actionQueryIds,
    diagnostics,
  }
}

function canonicalizeJson(value: unknown, depth = 0): string {
  if (depth > 12) return 'null'
  if (value === null || value === undefined) return 'null'
  if (typeof value === 'boolean' || typeof value === 'number') {
    return typeof value === 'number' && !Number.isFinite(value) ? 'null' : JSON.stringify(value)
  }
  if (typeof value === 'string') return JSON.stringify(value)
  if (Array.isArray(value)) {
    return `[${value.map((v) => canonicalizeJson(v, depth + 1)).join(',')}]`
  }
  if (typeof value === 'object') {
    const obj = value as Record<string, unknown>
    const keys = Object.keys(obj)
      .filter((k) => !FORBIDDEN_KEYS.has(k) && obj[k] !== undefined)
      .sort()
    return `{${keys.map((k) => `${JSON.stringify(k)}:${canonicalizeJson(obj[k], depth + 1)}`).join(',')}}`
  }
  return 'null'
}

function fnv1a32(input: string, seed: number): string {
  let h = seed >>> 0
  for (let i = 0; i < input.length; i++) {
    h ^= input.charCodeAt(i)
    h = Math.imul(h, 0x01000193) >>> 0
  }
  return h.toString(16).padStart(8, '0')
}

/**
 * Deterministic idempotency `request_id` for OpenUI mutations so re-dispatching the same
 * press `(deckId, sectionId, scriptRev, statementId, actionId, tool, canonicalArgs)` maps
 * to the exact same `deck_interact` idempotency key.
 */
export function deterministicOpenUIRequestId(parts: {
  deckId: string
  sectionId: string
  scriptRev: number
  statementId: string
  actionId: string
  tool: string
  args: Record<string, unknown>
}): string {
  const payload = [
    parts.deckId,
    parts.sectionId,
    String(parts.scriptRev),
    parts.statementId,
    parts.actionId,
    parts.tool,
    canonicalizeJson(parts.args),
  ].join('\u001f')
  const digest =
    fnv1a32(payload, 0x811c9dc5) +
    fnv1a32(`a:${payload}`, 0x9e3779b1) +
    fnv1a32(`b:${payload}`, 0x85ebca6b) +
    fnv1a32(`c:${payload}`, 0xc2b2ae35)
  const safeSec = parts.sectionId.replace(/[^A-Za-z0-9_-]/g, '').slice(0, 12) || 'sec'
  return `oui-${safeSec}-r${parts.scriptRev}-${digest}`
}

function sanitizeArgsObject(raw: unknown): Record<string, unknown> {
  if (raw === null || raw === undefined) return {}
  if (typeof raw !== 'object' || Array.isArray(raw)) {
    throw new Error('OpenUI tool arguments must be a plain object.')
  }
  const proto = Object.getPrototypeOf(raw)
  if (proto !== Object.prototype && proto !== null) {
    throw new Error('OpenUI tool arguments must be a plain object.')
  }
  const src = raw as Record<string, unknown>
  const out: Record<string, unknown> = {}
  for (const key of Object.keys(src)) {
    if (FORBIDDEN_KEYS.has(key)) {
      throw new Error(`Forbidden argument key "${key}".`)
    }
    const val = src[key]
    if (val !== undefined) out[key] = val
  }
  return out
}

export type DeckOpenUIProviderOptions = {
  deckId: string
  sectionId: string
  bindings: readonly Binding[]
  getSectionData: () => unknown
  getRuntime: () => DeckRuntimeStatus | null
  getActiveScript: () => ScriptRevision | null
  isPinned: () => boolean
  isInteractive: () => boolean
  invokeAction: (
    action: string,
    args: Record<string, unknown>,
    opts?: InvokeActionOptions,
  ) => Promise<InteractResponse | null>
}

/**
 * Runtime tool bridge for Deck OpenUI sections.
 * - Rejects unknown statements or tools not pinned in `bindings`
 * - Serves `deck_data` and `deck_status` locally with zero RPC round-trips
 * - Allows read-only declared Monty action `Query` calls (never pinned, never background `refresh` polling)
 * - Allows declared Monty action `Mutation` calls ONLY when `interactive && !pinned &&
 *   ctx.trigger === 'press' && ctx.action_id && activeScript.status === 'approved'`, with
 *   `expectedRev`, deterministic idempotency `requestId`, and private result/error propagation.
 */
export function createDeckOpenUIProvider(
  opts: DeckOpenUIProviderOptions,
): NonNullable<AppRendererProps['provider']> {
  const byId = new Map(opts.bindings.map((b) => [b.statement_id, b]))
  let querySeq = 0

  return async (tool, rawArgs, ctx) => {
    const binding = byId.get(ctx.statement_id)
    if (!binding || binding.tool !== tool) {
      throw new Error(`Unknown or unbound Deck OpenUI tool "${tool}" (${ctx.statement_id}).`)
    }

    const args = sanitizeArgsObject(rawArgs)

    if (binding.tool === DECK_DATA_TOOL) {
      if (binding.kind !== 'query') {
        throw new Error('deck_data can only be invoked as a Query.')
      }
      const data = opts.getSectionData()
      const path = typeof args.path === 'string' ? args.path.trim() : ''
      if (!path) return data ?? null
      const found = lookupPath(data, path)
      return found === undefined ? null : found
    }

    if (binding.tool === DECK_STATUS_TOOL) {
      if (binding.kind !== 'query') {
        throw new Error('deck_status can only be invoked as a Query.')
      }
      const rt = opts.getRuntime()
      return {
        deck_id: opts.deckId,
        state: rt?.state ?? 'stopped',
        stale: Boolean(rt?.stale),
        active_rev: rt?.active_rev ?? null,
        run_epoch: rt?.run_epoch ?? 0,
        step_count: rt?.step_count ?? 0,
        pinned: opts.isPinned(),
        interactive: opts.isInteractive(),
        last_error: rt?.last_error ?? null,
        last_ok_at: rt?.last_ok_at ?? null,
      }
    }

    // Remote Monty declared action path (strictly gated).
    const activeScript = opts.getActiveScript()
    if (!activeScript || activeScript.status !== 'approved') {
      throw new Error('Deck script must be approved before calling declared actions.')
    }
    const decl = activeScript.declared_actions.find((a) => a.name === binding.tool)
    if (!decl) {
      throw new Error(
        `Action "${binding.tool}" is not declared on approved script rev ${activeScript.rev}.`,
      )
    }

    if (binding.kind === 'query') {
      if (decl.mutates_state) {
        throw new Error(`Mutating action "${decl.name}" cannot be called from a Query.`)
      }
      if (opts.isPinned()) {
        throw new Error('Declared action queries are disabled while viewing a pinned snapshot.')
      }
      if (ctx.trigger === 'refresh') {
        throw new Error(`Automatic polling refresh is disabled for Monty action query "${decl.name}".`)
      }
      const actionKey = ctx.action_id?.trim() || `${ctx.trigger}-${++querySeq}`
      const requestId = deterministicOpenUIRequestId({
        deckId: opts.deckId,
        sectionId: opts.sectionId,
        scriptRev: activeScript.rev,
        statementId: ctx.statement_id,
        actionId: actionKey,
        tool: decl.name,
        args,
      })
      const res = await opts.invokeAction(decl.name, args, {
        expectedRev: activeScript.rev,
        requestId,
        privateOnly: true,
        silent: true,
        throwOnError: true,
      })
      if (!res) throw new Error(`Action "${decl.name}" did not return a response.`)
      if (!res.ok) throw new Error(res.error || `Action "${decl.name}" failed.`)
      return res.result ?? null
    }

    // Mutation path.
    if (!opts.isInteractive()) {
      throw new Error('Deck mutations require an interactive view.')
    }
    if (opts.isPinned()) {
      throw new Error('Deck mutations are disabled while viewing a pinned snapshot.')
    }
    if (ctx.trigger !== 'press') {
      throw new Error('Deck mutations require a user press activation.')
    }
    const actionId = typeof ctx.action_id === 'string' ? ctx.action_id.trim() : ''
    if (!actionId) {
      throw new Error('Deck mutations require a press action_id.')
    }

    const requestId = deterministicOpenUIRequestId({
      deckId: opts.deckId,
      sectionId: opts.sectionId,
      scriptRev: activeScript.rev,
      statementId: ctx.statement_id,
      actionId,
      tool: decl.name,
      args,
    })

    const res = await opts.invokeAction(decl.name, args, {
      expectedRev: activeScript.rev,
      requestId,
      privateOnly: true,
      silent: true,
      throwOnError: true,
    })
    if (!res) throw new Error(`Action "${decl.name}" did not return a response.`)
    if (!res.ok) throw new Error(res.error || `Action "${decl.name}" failed.`)
    return res.result ?? null
  }
}
