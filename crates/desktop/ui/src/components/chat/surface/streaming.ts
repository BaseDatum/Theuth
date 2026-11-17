import type { Spec } from '@json-render/core'
import { parsePartial } from './parse'

/**
 * fence-skel D4: the streaming reply without a last line that may still grow into a fence opener (a bare "`", "``",
 * "~~", or a fence marker with an info word and no newline yet: "```js" on its way to "```json-render"). Handed to
 * streamdown so it never draws a plain code card under a partial language; the line shows when its newline arrives.
 */
export function holdFenceOpener(text: string): string {
  const start = text.lastIndexOf('\n') + 1
  return /^ {0,3}(?:`+|~+|`{3,}[^`]*|~{3,}.*)$/.test(text.slice(start)) ? text.slice(0, start) : text
}

/** fence-skel D4: what an open json-render fence shows: this prefix's partial spec, else the last one that parsed. */
export function openSpec(code: string, kept: Spec | null): Spec | null {
  return parsePartial(code) ?? kept
}
