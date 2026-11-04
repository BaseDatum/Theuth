// 171F4 T2-8: every wrapper tag the engine writes into a model input that can land in a transcript renders as a card
// or nothing — never as a raw `<tag …>` bubble. A new wrapper must be classified by `injected.ts` or listed here.
import { readFileSync, readdirSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'
import { expect, test } from 'vitest'
import { isClassified } from './injected'

const ROOT = fileURLToPath(new URL('../../../../../', import.meta.url))
/** The crates that format model inputs (supervisor wakes, loop notes, cells, schedules, desks, studies, harnesses). */
const CRATES = ['agent', 'acp', 'autoresearch', 'cells', 'decision', 'double', 'dreaming', 'knowledge', 'meeting', 'roster', 'schedule', 'surface', 'talk', 'tasks', 'tools']

/** Tags written by those crates that never reach a transcript as a row of their own, each with the reason. */
const NOT_A_ROW: Record<string, string> = {
  b: "HTML inside a python cell's display data (cells/python.rs), not a wrapper",
  change: 'a line inside `<notebook_changes>`, which is hidden whole',
  item: 'an element inside `<notebook>` / `<notebook_changes>`, hidden whole',
  description: 'an element inside `<notebook>`, hidden whole',
  chat: "an endless parent's view, cut off by `splitParentView` (ParentViewCard)",
  concept: "the knowledge harvest's side-model prompt and answer, never a session row",
  conversation: "the vision side model's question (vision.rs), never a session row",
  vision_observation: 'stored with typed `vision` metadata; the reducer renders MessageVision from it',
  request: 'inside the `<schedule>` envelope: the schedule card body',
  until: 'inside the `<schedule>` envelope: the schedule card body',
  when: 'inside the `<schedule>` envelope: the schedule card body',
  task_result: "opencode's own output, parsed by the acp harness, never written",
  task_error: "opencode's own output, parsed by the acp harness, never written",
  workflow: "a section of the advisor's system prompt, never a session row",
  // Not fixed in 171F4 (docs/design/171f4-notes-d.md): the opening brief of an autoresearch experiment's own session.
  study: "an experiment session's opening brief (autoresearch/brief.rs) followed by instructions; see the 171F4-D notes",
}

function rustFiles(dir: string): string[] {
  return readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
    const p = join(dir, e.name)
    if (e.isDirectory()) return e.name === 'tests' ? [] : rustFiles(p)
    return e.name.endsWith('.rs') && !/(^|_)tests?\.rs$/.test(e.name) ? [p] : []
  })
}

/** Wrapper names per file: a string literal opening `"<name` (then a space, `>` or an escape) whose `</name>` closes in a
 * string literal of the same file; test modules excluded. */
function wrappers(): Map<string, string> {
  const out = new Map<string, string>()
  for (const krate of CRATES)
    for (const f of rustFiles(join(ROOT, 'crates', krate, 'src'))) {
      let s = readFileSync(f, 'utf8')
      const t = s.search(/\n#\[cfg\(test\)\]\n(pub(\(crate\))? )?mod tests/)
      if (t >= 0) s = s.slice(0, t)
      const closes = new Set([...s.matchAll(/"(?:[^"\\\n]|\\.)*<\/([a-z][a-z_]*)>/g)].map((m) => m[1]))
      for (const m of s.matchAll(/"<([a-z][a-z_]*)(?=[ >\\])/g)) if (closes.has(m[1] as string)) out.set(m[1] as string, f.slice(ROOT.length))
    }
  return out
}

test('171F4 T2-8: every engine wrapper is a card, hidden, or listed with its reason', () => {
  const found = wrappers()
  // The scan sees the wrappers it must (a regression in the scan would pass everything).
  for (const name of ['child_stalled', 'peer_state', 'child_lost', 'job_result', 'cell_result', 'reminder']) expect(found.has(name), name).toBe(true)
  const raw = [...found].filter(([name]) => !isClassified(name) && !Object.hasOwn(NOT_A_ROW, name)).map(([name, file]) => `<${name}> in ${file}`)
  expect(raw, 'render these (injected.ts CARD/HIDDEN) or list them in NOT_A_ROW with the reason').toEqual([])
  // No stale entries: every listed tag is still written somewhere and not also classified.
  for (const name of Object.keys(NOT_A_ROW)) {
    expect(found.has(name), `${name} is no longer written`).toBe(true)
    expect(isClassified(name), `${name} is classified now`).toBe(false)
  }
})
