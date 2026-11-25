// 215 wizard: the copy that carries meaning (subscriptions, key providers, starter prompts) and the pure choices behind
// the steps, kept out of the components so they are tested without a DOM.
import { providerName } from '@/components/chat/error-card'
import { BUILTIN_LABEL } from '@/components/settings/providers'

/** A provider id as people write it: `anthropic` → `Anthropic`, `opencode` → `OpenCode Zen`, `ollama` → `Ollama`. */
export const providerLabel = (id: string): string => BUILTIN_LABEL[id] ?? providerName(id)

export type ConnectMode = 'subscription' | 'key' | 'local'

/** What each way of connecting means, three short facts each (the Connect step's side panel). */
export const MODE_FACTS: Record<ConnectMode, { title: string; facts: readonly string[] }> = {
  subscription: {
    title: 'A plan you already pay for',
    facts: ['No per-token bill: usage counts against your plan.', 'Limits reset on your plan\u2019s schedule; Costs shows where you stand.', 'You sign in once in the browser; the tokens stay on this computer.'],
  },
  key: {
    title: 'Pay per use',
    facts: ['Billed by the provider for exactly what you use.', 'Costs shows every call and its price.', 'The key is encrypted on this computer and never shown again.'],
  },
  local: {
    title: 'Runs on this computer',
    facts: ['Nothing leaves your machine; works offline.', 'Free to run; speed depends on your hardware.', 'Ollama, LM Studio, llama.cpp and others are found automatically.'],
  },
}

/** A subscription sign-in: `login(ns)`; `provider` is the model provider it unlocks in `list_models`. */
export type Subscription = { ns: string; label: string; plan: string; provider: string }

export const SUBSCRIPTIONS: readonly Subscription[] = [
  { ns: 'anthropic', label: 'Claude Pro / Max', plan: 'Sign in with your Claude account', provider: 'anthropic' },
  { ns: 'openai', label: 'ChatGPT Plus / Pro', plan: 'Sign in with your ChatGPT account', provider: 'openai' },
]
export const MORE_SUBSCRIPTIONS: readonly Subscription[] = [
  { ns: 'xai', label: 'SuperGrok', plan: 'Sign in with your xAI account', provider: 'xai' },
  { ns: 'cursor', label: 'Cursor', plan: 'Sign in with your Cursor account', provider: 'cursor' },
]

/** A sign-in waiting on the browser (`url` once `login` answered), and the last one that failed. */
export type Login = { pending: { ns: string; url: string | null } | null; failed: { ns: string; message: string } | null }

/**
 * `LoginFinished` folded into the sign-in state: only the event for the sign-in this step started counts (another
 * window's or a stale one is ignored, and `next` is then `s` itself). `provider` is the connected model provider.
 */
export function loginFinished(s: Login, e: { provider: string; error?: string | null }): { next: Login; provider: string | null } {
  if (!s.pending || e.provider !== s.pending.ns) return { next: s, provider: null }
  if (e.error) return { next: { pending: null, failed: { ns: e.provider, message: e.error } }, provider: null }
  return { next: { pending: null, failed: null }, provider: [...SUBSCRIPTIONS, ...MORE_SUBSCRIPTIONS].find((x) => x.ns === e.provider)?.provider ?? e.provider }
}

/** An API key provider: `secret` is the name the engine reads the key from (`set_secret`): the adapter's, or the built-in key provider's. */
export type KeyProvider = { secret: string; label: string; provider: string; placeholder: string }

export const KEY_PROVIDERS: readonly KeyProvider[] = [
  { secret: 'anthropic', label: 'Anthropic', provider: 'anthropic', placeholder: 'sk-ant-…' },
  { secret: 'openai', label: 'OpenAI', provider: 'openai', placeholder: 'sk-…' },
  { secret: 'openrouter', label: 'OpenRouter', provider: 'openrouter', placeholder: 'sk-or-…' },
  { secret: 'opencode', label: 'OpenCode', provider: 'opencode', placeholder: 'Your OpenCode Zen key' },
  { secret: 'google', label: 'Google', provider: 'google', placeholder: 'AIza…' },
]

export const keyNote = (p: KeyProvider): string => `${p.label} API key (added during setup)`

type Model = { id: string; name: string; provider: string; context: number }

/** Not chat models: embeddings, speech, images, moderation. */
const NOT_CHAT = /embed|tts|whisper|transcribe|audio|realtime|image|dall-e|moderation|search-preview|computer-use/i
/** The flagship lines first, the small and fast ones last: names, not prices, so it holds as catalogs change. */
const STRONG = /opus|sonnet|gpt-5(?!.*(mini|nano))|o3(?!-mini)|gemini-[\d.]+-pro|grok-4|kimi|glm-4|qwen3|deepseek/i
const LIGHT = /mini|nano|haiku|flash|lite|small|8b|7b|3b/i

/** Higher is a better default: flagship over light, a bigger context window breaking ties. */
function score(m: Model): number {
  const name = `${m.id} ${m.name}`
  return (STRONG.test(name) ? 2 : 0) - (LIGHT.test(name) ? 1 : 0) + Math.min(m.context, 2_000_000) / 10_000_000
}

/**
 * Three to five models to offer as the default: chat models only, the `prefer` provider's first (the one just
 * connected), at most two per provider so the choice spans what is connected, the current default kept in view.
 */
export function recommendModels<M extends Model>(models: readonly M[], prefer: string | null, current: string | null | undefined, n = 4): M[] {
  const chat = models.filter((m) => !NOT_CHAT.test(m.id))
  const ranked = [...chat].sort((a, b) => Number(b.provider === prefer) - Number(a.provider === prefer) || score(b) - score(a) || a.id.localeCompare(b.id))
  const per = new Map<string, number>()
  const picked: M[] = []
  const now = chat.find((m) => m.id === current)
  if (now) {
    picked.push(now)
    per.set(now.provider, 1)
  }
  for (const m of ranked) {
    if (picked.length >= n) break
    if (picked.includes(m) || (per.get(m.provider) ?? 0) >= 2) continue
    picked.push(m)
    per.set(m.provider, (per.get(m.provider) ?? 0) + 1)
  }
  // Fewer than three across the spread: fill from the ranking regardless of provider.
  for (const m of ranked) {
    if (picked.length >= Math.min(3, chat.length)) break
    if (!picked.includes(m)) picked.push(m)
  }
  return picked
}

/** `200k`, `1M`, or nothing when the engine does not know the window. */
export function contextLabel(tokens: number): string {
  if (tokens <= 0) return ''
  if (tokens >= 1_000_000) return `${+(tokens / 1_000_000).toFixed(1)}M context`
  return `${Math.round(tokens / 1000)}k context`
}

/** The providers behind a model list, in first-seen order. */
export const connectedProviders = (models: readonly { provider: string }[]): string[] => [...new Set(models.map((m) => m.provider))]

/** A model's facts for the Default model step's side panel. */
export function modelFacts(m: { provider: string; context: number; reasoning: boolean; image: boolean; local: boolean; efforts: readonly string[] }): string[] {
  return [
    m.local ? 'Runs on this computer' : `Served by ${providerLabel(m.provider)}`,
    m.context > 0 ? `Reads ${contextLabel(m.context).replace(' context', '')} tokens of context at once` : 'Context window not reported',
    m.reasoning ? (m.efforts.length > 0 ? `Thinks before answering (${m.efforts.join(', ')})` : 'Thinks before answering') : 'Answers directly, without a thinking phase',
    m.image ? 'Reads images and screenshots' : 'Text only',
  ]
}

/** A few seconds of slack between the dialog's clock and the engine's stamp (same machine, separate processes). */
const PAIR_SLACK_MS = 5_000

/**
 * Whether a just-paired device is the phone this dialog's code paired: paired after the dialog opened, and not another
 * theuth node (`platform` `theuth` is a computer joining the mesh). The link events carry no pairing id to match.
 */
export function isNewPhone(d: { paired_ms: number; platform: string }, openedAt: number): boolean {
  return d.platform !== 'theuth' && d.paired_ms >= openedAt - PAIR_SLACK_MS
}

export type StarterId = 'tour' | 'page' | 'research' | 'project'

/** A first-chat card; `input` cards fill `{}` from their field before sending. */
export type Starter = { id: StarterId; title: string; blurb: string; /** What lands in the chat and the Library. */ yields: readonly string[]; input?: { label: string; placeholder: string } }

export const STARTERS: readonly Starter[] = [
  { id: 'tour', title: 'Tour this computer', blurb: 'What is on this machine and where the space went, drawn as one picture.', yields: ['A read-only look at the disk, apps and memory', 'One diagram of where the space went', 'Two or three safe ways to free some up'] },
  { id: 'page', title: 'Make me a page', blurb: 'A calm start-of-day dashboard as one HTML file, built and shown to you.', yields: ['One self-contained HTML file in your Library', 'Shown to you as soon as it is built', 'Light and dark, no accounts needed'] },
  { id: 'research', title: 'Research something', blurb: 'A short briefing from primary sources, every claim linked.', yields: ['A briefing built from primary sources', 'Every claim linked to where it came from', 'The three sources worth reading next'], input: { label: 'Topic', placeholder: 'e.g. solid-state batteries' } },
  { id: 'project', title: 'Explain a project', blurb: 'Point at a folder and get a map of how it works.', yields: ['An overview board of the project', 'A diagram of how the pieces fit', 'Where to start to change something'], input: { label: 'Folder', placeholder: '/Users/you/code/project' } },
]

/** The prompt each card sends; `value` is the card's field (topic or folder), for the tour the machine's noun (`Mac`). */
export function starterPrompt(id: StarterId, value = ''): string {
  const v = value.trim()
  switch (id) {
    case 'tour':
      return [
        `Give me a tour of this ${v || 'computer'}.`,
        'Look at what it is (model, processor, memory, OS version), what is installed and running, how full the disk is, and which folders in my home directory take the most space.',
        'Then draw it as one picture I can take in at a glance: a diagram or chart with the five biggest folders called out, followed by two or three concrete, safe ways to free up space.',
        'Only look; do not change, move or delete anything.',
      ].join(' ')
    case 'page':
      return [
        'Make me a single self-contained HTML page I can keep open in a browser tab: a calm start-of-day dashboard.',
        'Show today\'s date and a large, readable clock, a short quote worth reading, a focus list of three things I can tick off (saved in the browser so it survives a reload), and a two-minute breathing timer for breaks.',
        'No accounts, no external services, no build step. Make it genuinely beautiful in both light and dark mode, save it, and show it to me.',
      ].join(' ')
    case 'research':
      return [
        `Research ${v} for me.`,
        'Search the web and read the most useful primary sources, not summaries of summaries.',
        'Write a clear briefing: what it is, why it matters right now, where informed people disagree, and what is likely to happen next.',
        'Cite every claim with a link, and finish with the three sources you trust most and why.',
      ].join(' ')
    case 'project':
      return [
        `Explain the project in ${v}.`,
        'Read its README, layout, entry points and the code that matters, then give me an overview board: what it does, how the pieces fit together (with a diagram), how to build and run it, and where I would start if I wanted to change something.',
        'Only read; do not modify any files.',
      ].join(' ')
  }
}
