// 197 §3: the engine's surface-fence scanner (`surface_fences`, `crates/agent/src/context.rs`), ported line for line so a
// fence index the engine issues names the fence the transcript draws. A fence opens on a line that starts (no indent)
// with three or more backticks or tildes immediately followed by its info string (a backtick opener's info holds no
// backtick). It closes at the next line made of the same character, at least as many, then only whitespace; an unclosed
// fence runs to the end of the text. Only info strings exactly `json-render`, `openui` or `mermaid` (trailing whitespace
// aside) are surface fences and counted. Every other fence (```rust, a bare ```) is skipped whole: a surface-looking
// line inside it is content. `\r\n` line ends are accepted.

export type SurfaceLanguage = 'json-render' | 'openui' | 'mermaid'

/** One surface fence: its language and its body (the text between the fence lines, as the engine slices it). */
export type SurfaceFence = { language: SurfaceLanguage; body: string }

const SURFACE: readonly string[] = ['json-render', 'openui', 'mermaid']

/** The surface fences of one text, in order; `index` in a `fence` target is the position in this list. */
export function surfaceFences(text: string): SurfaceFence[] {
  const out: SurfaceFence[] = []
  let open: { c: string; run: number; info: string; body: number } | null = null
  let at = 0
  for (const line of text.split(/(?<=\n)/)) {
    const start = at
    at += line.length
    const bare = line.replace(/[\r\n]+$/, '')
    const lead = (c: string) => {
      let n = 0
      while (bare[n] === c) n++
      return n
    }
    if (!open) {
      const c = bare[0]
      if (c !== '`' && c !== '~') continue
      const run = lead(c)
      const info = bare.slice(run).trimEnd()
      if (run >= 3 && !(c === '`' && info.includes('`'))) open = { c, run, info, body: at }
      continue
    }
    const n = lead(open.c)
    if (n >= open.run && bare.slice(n).trim() === '') {
      if (SURFACE.includes(open.info)) out.push({ language: open.info as SurfaceLanguage, body: text.slice(open.body, start) })
      open = null
    }
  }
  if (open && SURFACE.includes(open.info)) out.push({ language: open.info as SurfaceLanguage, body: text.slice(open.body) })
  return out
}
