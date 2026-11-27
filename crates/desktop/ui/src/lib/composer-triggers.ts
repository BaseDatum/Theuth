// Inline composer triggers (`$` for skills, `&` for saved prompts): pure token detection,
// text replacement, and client-side filtering so Composer.tsx stays thin.

import type { Suffix } from '@/lib/suffixes'
import type { SkillMeta } from '@/rpc/bindings/method.agent.output'

export type InlineTriggerKind = '$' | '&'
export type InlineTrigger = { kind: InlineTriggerKind; start: number; word: string }

/** Odd number of ``` fence lines before the current line → inside a fenced code block (same check as pathTrigger). */
function inFencedBlock(before: string): boolean {
  const lineStart = before.lastIndexOf('\n') + 1
  const fences = before
    .slice(0, lineStart)
    .split('\n')
    .filter((l) => l.trimStart().startsWith('```')).length
  return fences % 2 === 1
}

/**
 * The `$word` skill trigger under `caret` when `$` starts a word (start of input or after whitespace).
 * Stops before `/`, whitespace, or another `$` (`$HOME/` returns null so PATH-AC can take over; `${` and `$?` do not open).
 */
export function skillTrigger(text: string, caret: number): { start: number; word: string } | null {
  const before = text.slice(0, caret)
  if (inFencedBlock(before)) return null
  const m = /(^|\s)\$([A-Za-z0-9._-]*)$/.exec(before)
  if (!m) return null
  const word = m[2] ?? ''
  return { start: before.length - word.length - 1, word }
}

/**
 * The `&word` prompt trigger under `caret` when `&` starts a word (start of input or after whitespace).
 * Matches a non-whitespace run without `/`; space ends the token and closes the popup.
 */
export function promptTrigger(text: string, caret: number): { start: number; word: string } | null {
  const before = text.slice(0, caret)
  if (inFencedBlock(before)) return null
  const m = /(^|\s)&([^\s/]*)$/.exec(before)
  if (!m) return null
  const word = m[2] ?? ''
  return { start: before.length - word.length - 1, word }
}

/** Either inline trigger (`$` skill or `&` prompt) under `caret`, or `null`. */
export function inlineTrigger(text: string, caret: number): InlineTrigger | null {
  const s = skillTrigger(text, caret)
  if (s) return { kind: '$', ...s }
  const p = promptTrigger(text, caret)
  if (p) return { kind: '&', ...p }
  return null
}

/** Remove the prefix character + `word` + one trailing space (same contract as `stripAt`). */
export function stripTrigger(text: string, start: number, word: string): string {
  const end = start + 1 + word.length
  return text.slice(0, start) + text.slice(text[end] === ' ' ? end + 1 : end)
}

/**
 * Replace the trigger token with `expansion.trim()` and return the updated text and caret (end of inserted text).
 * If `expansion` is empty/blank, strips the token. Inserts a single separating space when the next char is non-whitespace.
 */
export function insertExpansion(
  text: string,
  start: number,
  word: string,
  expansion: string,
): { text: string; caret: number } {
  const body = expansion.trim()
  if (!body) return { text: stripTrigger(text, start, word), caret: start }
  const end = start + 1 + word.length
  const after = text.slice(end)
  const sep = after.length > 0 && !/^\s/.test(after) ? ' ' : ''
  return {
    text: text.slice(0, start) + body + sep + after,
    caret: start + body.length,
  }
}

const normSkill = (s: string) => s.toLowerCase().replace(/_/g, '-')

/**
 * Client-side filter for `$` skill autocomplete:
 * case-insensitive, `_` ≡ `-`; ranks name prefix → name substring → description substring;
 * digit-only queries match skill names only (so `$100` does not pop on a description hit).
 * Preserves canonical `SkillMeta.name` and index order within each rank.
 */
export function filterSkillTriggers(skills: readonly SkillMeta[], query: string): SkillMeta[] {
  if (query === '') return [...skills]
  const q = normSkill(query)
  const digitsOnly = /^\d+$/.test(query)
  const prefix: SkillMeta[] = []
  const nameSub: SkillMeta[] = []
  const descSub: SkillMeta[] = []
  for (const s of skills) {
    const name = normSkill(s.name)
    if (name.startsWith(q)) prefix.push(s)
    else if (name.includes(q)) nameSub.push(s)
    else if (!digitsOnly && normSkill(s.description ?? '').includes(q)) descSub.push(s)
  }
  return [...prefix, ...nameSub, ...descSub]
}

const normPrompt = (s: string) => s.toLowerCase().replace(/[\s_-]+/g, ' ')

/**
 * Client-side filter for `&` prompt autocomplete:
 * empty query lists all prompts in saved order; otherwise case-insensitive with space, `_`, and `-`
 * treated as the same separator, ranking name-prefix matches above name-substring matches.
 */
export function filterPromptTriggers(prompts: readonly Suffix[], query: string): Suffix[] {
  if (query === '') return [...prompts]
  const q = normPrompt(query)
  const prefix: Suffix[] = []
  const sub: Suffix[] = []
  for (const p of prompts) {
    const name = normPrompt(p.name)
    if (name.startsWith(q)) prefix.push(p)
    else if (name.includes(q)) sub.push(p)
  }
  return [...prefix, ...sub]
}
