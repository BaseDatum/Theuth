// Which harness runs a subagent, as a 14 px single-colour line glyph in the lucide style (24 viewBox, stroke =
// currentColor, round caps, no fill), and its model as a compact label. Simplified marks drawn here, not brand logos.

import { Bot, Terminal } from 'lucide-react'
import type { ReactNode, SVGProps } from 'react'

export type HarnessKey = 'theuth' | 'omp' | 'claude' | 'opencode' | 'codex' | 'terminal'

/** Prefix → glyph, checked in order; `theuth` last so a harness merely named after it still matches its own prefix first. */
const PREFIXES: readonly [string, HarnessKey][] = [
  ['omp', 'omp'],
  ['claude', 'claude'],
  ['opencode', 'opencode'],
  ['codex', 'codex'],
  ['theuth', 'theuth'],
]

/**
 * The glyph for a subagent's harness: `agent` is the `agents.yaml` key (`null` / empty = theuth itself), or a watch-only
 * child's `ObservedMeta.source`. Case-insensitive prefix match, an `@node` suffix ignored — `claude-acp`, `opencode-v2`,
 * `omp@P50`, `omp_task`, `opencode_ext` all resolve; anything else is the generic terminal.
 */
export function harnessKey(agent: string | null | undefined): HarnessKey {
  const key = (agent ?? '').trim().toLowerCase().split('@')[0] ?? ''
  if (!key) return 'theuth'
  return PREFIXES.find(([prefix]) => key.startsWith(prefix))?.[1] ?? 'terminal'
}

type Glyph = (props: { className?: string }) => ReactNode

const line: SVGProps<SVGSVGElement> = {
  viewBox: '0 0 24 24',
  fill: 'none',
  stroke: 'currentColor',
  strokeWidth: 1.75,
  strokeLinecap: 'round',
  strokeLinejoin: 'round',
  'aria-hidden': true,
}

const GLYPHS: Record<HarnessKey, Glyph> = {
  theuth: ({ className }) => <Bot className={className} strokeWidth={1.75} aria-hidden />,
  terminal: ({ className }) => <Terminal className={className} strokeWidth={1.75} aria-hidden />,
  // A π: omp is oh-my-pi.
  omp: ({ className }) => (
    <svg {...line} className={className}>
      <path d="M5 7.5h14" />
      <path d="M9 7.5V18" />
      <path d="M15 7.5v8.5a2 2 0 0 0 2 2" />
    </svg>
  ),
  // A starburst: four strokes through the centre.
  claude: ({ className }) => (
    <svg {...line} className={className}>
      <path d="M12 3v18" />
      <path d="M3 12h18" />
      <path d="m5.6 5.6 12.8 12.8" />
      <path d="m18.4 5.6-12.8 12.8" />
    </svg>
  ),
  // Code brackets.
  opencode: ({ className }) => (
    <svg {...line} className={className}>
      <path d="m8 7-5 5 5 5" />
      <path d="m16 7 5 5-5 5" />
    </svg>
  ),
  // A hexagon holding a prompt.
  codex: ({ className }) => (
    <svg {...line} className={className}>
      <path d="M12 2.5 20.5 7.25v9.5L12 21.5l-8.5-4.75v-9.5Z" />
      <path d="m8.5 9.5 2.5 2.5-2.5 2.5" />
      <path d="M13 14.5h3" />
    </svg>
  ),
}

/** The harness glyph of `agent` (see `harnessKey`). */
export function HarnessGlyph({ agent, className }: { agent: string | null | undefined; className?: string }) {
  const Glyph = GLYPHS[harnessKey(agent)]
  return <Glyph className={className} />
}

/**
 * A model id as a short label: no provider path, no date or `-latest` suffix, `claude-` dropped, `5-5`-style versions
 * dotted, `deepseek` → `ds`, lower case. `anthropic/claude-opus-5-5` → `opus-5.5`,
 * `rwx/deepseek-ai/DeepSeek-V4.1-Flash` → `ds-v4.1-flash`; empty stays empty.
 */
export function shortModel(model: string | null | undefined): string {
  let m = (model ?? '').trim().toLowerCase()
  m = m.slice(m.lastIndexOf('/') + 1).split('@')[0] ?? ''
  m = m
    .replace(/-(\d{8}|\d{4}-\d{2}-\d{2}|latest)$/, '')
    .replace(/^claude-/, '')
    .replace(/^deepseek/, 'ds')
    .replace(/(^|-)(\d+)-(\d)(?=$|-)/g, '$1$2.$3')
  return m
}
