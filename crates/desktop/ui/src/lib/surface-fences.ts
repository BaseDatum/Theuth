// 197 §3: the surface-fence rule, shared with the engine's `Fence` targets (one rule, so a fence index the engine
// issues names the fence the transcript draws). A surface fence opens on a line that is three or more backticks or
// tildes followed immediately by `json-render`, `openui` or `mermaid` and nothing else but whitespace; it closes at the
// next line made only of the same character, at least as long (trailing whitespace allowed); an unclosed fence runs to
// the end of the text. Any other fence (```rust, ~~~ …) is ignored: not tracked, not counted. No indentation: a fence
// streamdown finds inside a list item is not one here, so the counts differ and the transcript offers no control.

export type SurfaceLanguage = 'json-render' | 'openui' | 'mermaid'

/** One surface fence: its language and its body (the lines between the opener and the closer, without either). */
export type SurfaceFence = { language: SurfaceLanguage; body: string }

const OPEN = /^(`{3,}|~{3,})(json-render|openui|mermaid)\s*$/

/** The surface fences of one text, in order; `index` in a `fence` target is the position in this list. */
export function surfaceFences(text: string): SurfaceFence[] {
  const out: SurfaceFence[] = []
  let open: { fence: string; language: SurfaceLanguage; body: string[] } | null = null
  for (const line of text.split('\n')) {
    if (open) {
      const t = line.trimEnd()
      if (t.length >= open.fence.length && t === open.fence[0]!.repeat(t.length)) {
        out.push({ language: open.language, body: open.body.join('\n') })
        open = null
      } else open.body.push(line)
      continue
    }
    const m = OPEN.exec(line)
    if (m) open = { fence: m[1]!, language: m[2] as SurfaceLanguage, body: [] }
  }
  if (open) out.push({ language: open.language, body: open.body.join('\n') })
  return out
}
