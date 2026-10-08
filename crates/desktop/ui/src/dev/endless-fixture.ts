// 137 §11.3: the endless RPCs answered from memory so the ∞ badge and the Memory panel render against an engine that
// predates `session_memory` / `endless_zoom` / `endless_recall`. Dev only, behind `VITE_ENDLESS_FIXTURE=1` (or
// `localStorage.endless-fixture = "1"`); with it on, every session is treated as endless. The connection is still the engine's.

import type { Api } from '@/rpc/api'
import type { EndlessConfig, SessionMemory, ViewLine } from '@/rpc/endless-stubs'

/** `"1"` turns the fixture on; `"backfill"` also serves the R1-D4 capped-backfill memory. `null` = off. */
const flag = (): string | null => (import.meta.env.DEV ? globalThis.localStorage?.getItem('endless-fixture') || import.meta.env.VITE_ENDLESS_FIXTURE || null : null)
export const endlessFixtureOn = (): boolean => !!flag()

const line = (id: number, n: number, text: string, built = true): ViewLine => ({ id, n, text, built, level: Math.log2(n) })

/** A 71-line view in the shape the fold produces: coarse bands oldest first, then one line per recent message. */
const view: ViewLine[] = [
  line(0, 64, 'user asks for a summary-tree memory mode; talk sketches the tree, the fixed view and a zoom tool; work: lab replay over two desk logs shows the shape holds (84–96 % of view bytes stable per turn).'),
  line(64, 32, 'user: name it endless; talk: D1–D15 drafted, slack fold at 12.5 %, compactor context frozen per turn; tool: cargo test -p agent green on fold invariants.'),
  line(96, 32, 'work: guards G1–G5 written (copy, grounding, status words, format, non-empty); echo: the SCALE-copy regression line kept as a fixture; note: retries stay in one conversation.'),
  line(128, 16, 'user wants the New chat switch with the ∞ label; talk agrees on the Private option as the quality bar.'),
  line(144, 16, 'tool: list_sessions shows 212 rows; work: SessionSummary.memory added so the sidebar can draw the glyph.'),
  line(160, 8, 'user: how much does the compactor cost? talk: Haiku-class $0.015 per node with the frozen context, output tokens ~37 % of it.'),
  line(168, 8, 'work: settle wait capped at 45 s; past it unbuilt short units render verbatim, long ones as "(not summarized yet: zoom it)".'),
  line(176, 4, 'user pastes the engine log; tool: grep finds the view_rebase counter at turn 31, 3 blocks hit.'),
  line(180, 4, 'talk: the Memory panel groups the view by band and zooms a line into its two children.'),
  line(184, 2, 'user: make the badge pulse only while settling; talk: motion-safe only.'),
  line(186, 2, 'tool: vitest run NewChatPane — 3 passed.'),
  line(188, 1, 'user: and a Settings → Memory page with the view size slider.'),
  line(189, 1, 'talk: 16–256 KB, step 8, with ≈ bytes ÷ 3 tokens beside it.'),
  line(190, 1, 'tool: tsc --noEmit — 0 errors.'),
  line(191, 1, 'user: ship it while I sleep.', false),
]

const units: Record<number, string> = {
  184: 'Make the badge pulse only while the compactor is settling — a steady badge otherwise, please.',
  185: 'Done: `motion-safe:animate-pulse` keyed on `settling`; reduced-motion users get a static glyph.',
  186: '$ bunx vitest run src/components/chat/NewChatPane.test.ts\n ✓ CreateOptions renders the ∞ switch\n ✓ the panel appears when on\n ✓ createOptions passes memory\n\n Test Files  1 passed (1)\n      Tests  3 passed (3)',
  187: 'Three green. Moving to the header badge.',
  188: 'And a Settings → Memory page with the view size slider.',
  189: '16–256 KB, step 8, with ≈ bytes ÷ 3 tokens beside it.',
  190: '$ bunx tsc --noEmit -p .\n(no output)',
  191: 'Ship it while I sleep.',
}

const children = (id: number, n: number): ViewLine[] => {
  if (n === 1) return [line(id, 1, units[id] ?? `user: message ${id} in full — ${view.find((l) => id >= l.id && id < l.id + l.n)?.text ?? ''}`)]
  const half = n / 2
  const parent = view.find((l) => l.id === id && l.n === n)?.text ?? `${id}+${n}`
  return [line(id, half, `${parent.split(';')[0]}.`), line(id + half, half, `${parent.split(';').slice(1).join(';').trim() || parent}`)]
}

const memory: SessionMemory = {
  mode: 'endless',
  view,
  settling: true,
  unbuilt: 1,
  units: 192,
  stats: {
    calls: 311,
    guard_trips: { copy: 4, grounding: 2, status: 1, format: 3, empty: 0, size: 6 },
    input_tokens: 1_204_000,
    cache_read_tokens: 1_150_000,
    cache_write_tokens: 42_000,
    output_tokens: 61_000,
    usd_estimate: 4.37,
  },
  model: 'anthropic/claude-haiku-4-5',
  backfill: null,
}

/** `localStorage.endless-fixture = "backfill"`: the R1-D4 capped-backfill shape — sentinel first line, projected cost in the footer. */
const backfilling: SessionMemory = {
  ...memory,
  settling: false,
  unbuilt: 0,
  view: [line(0, 2_412, '(earlier history, not summarized; recall searches it)', false), ...view.map((l) => ({ ...l, id: l.id + 2_412 }))],
  backfill: { done: 812, total: 2_000, eta_secs: 3_540, usd_estimate: 2.9 },
}

let settings: EndlessConfig = {
  default_chat: false,
  default_desk: true,
  default_meeting: false,
  view_bytes: 128_000,
  meeting_view_bytes: 32_000,
  node_bytes: 512,
  slack: 0.125,
  settle_secs: 45,
  jobs: 8,
  tries: 5,
  retry_secs: 10,
  cap_chars: 30_000,
  backfill_max_units: 2_000,
  model: null,
}

const wrapped = new WeakMap<Api, Api>()

/** The engine's `Api` with the 137 methods answered here when the fixture is on; the same object otherwise. One wrapper per `api` (`useApi` callers key effects on it). */
export function endlessFixture(api: Api): Api {
  if (!endlessFixtureOn()) return api
  const hit = wrapped.get(api)
  if (hit) return hit
  const fixture: Api = {
    ...api,
    // The two newest root chats (not desks, not archived) draw the sidebar's ∞ glyph (`SessionSummary.memory`).
    list_sessions: () =>
      api.list_sessions().then((rows) => {
        let left = 2
        return rows.map((r) => (r.parent === null && !r.archived && !r.desk && left-- > 0 ? { ...r, memory: 'endless' as const } : r))
      }),
    session_memory: () => Promise.resolve(flag() === 'backfill' ? backfilling : memory),
    endless_zoom: (_s, id, n) => Promise.resolve(children(id, n)),
    endless_recall: (_s, query) => {
      const q = query.toLowerCase()
      return Promise.resolve(
        Object.entries(units)
          .filter(([, t]) => t.toLowerCase().includes(q))
          .map(([id, t]) => line(Number(id), 1, t.split('\n')[0] ?? t)),
      )
    },
    set_session_memory: () => Promise.resolve(memory),
    endless_settings_get: () => Promise.resolve(settings),
    endless_settings_set: (next) => {
      settings = next
      return Promise.resolve(settings)
    },
  }
  wrapped.set(api, fixture)
  return fixture
}
