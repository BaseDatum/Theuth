// `crates/surface/src/anchor.rs` in TS (design 71 §2.3): what a `code:` link, a `GraphNode.ref`, a `Sequence` step
// `ref` and an `Excerpt` path have in common. Same grammar, same rejects, same messages, so the desktop and the TUI
// refuse the same fences and the board reader (step 4) resolves the same anchors.
//
//   Anchor      := CodeAnchor | BlockAnchor
//   CodeAnchor  := <path>#L<a>-L<b> | <path>#L<a> | <path>     root-relative, 1-based, inclusive
//   BlockAnchor := block:<blockId>

/** `#L<a>` parses as `lines: [a, a]`, like the Rust `Option<(u32, u32)>`. */
export type Anchor = { kind: 'code'; path: string; lines?: [number, number] } | { kind: 'block'; id: string }

/** A root-relative path, or the message a located error carries (`Excerpt.path` and the path half of a `CodeAnchor`). */
export function checkPath(path: string): string | null {
  if (path === '') return 'empty path'
  if (path.startsWith('/') || path.startsWith('\\')) return `path must be root-relative, not absolute: ${JSON.stringify(path)}`
  const scheme = /^([A-Za-z][A-Za-z0-9+.-]*):/.exec(path)?.[1]
  if (scheme) return `path must not carry a scheme: ${scheme}:`
  if (path.includes(':')) return `path must not contain ':'; a line is #L<n>, not :<n>: ${JSON.stringify(path)}`
  if (path.includes('#')) return "path must not contain '#'; the line range follows a single '#'"
  if (/[^\S ]/.test(path)) return 'path must not contain line breaks or tabs'
  for (const seg of path.split('/')) {
    if (seg === '') return `empty path segment in ${JSON.stringify(path)}`
    if (seg === '.' || seg === '..') return `path must not contain ${JSON.stringify(seg)}: ${JSON.stringify(path)}`
  }
  return null
}

/** `L<a>` or `L<a>-L<b>`, `1 ≤ a ≤ b`. */
function parseLines(f: string): [number, number] | string {
  const bad = `expected #L<a> or #L<a>-L<b>, got #${f}`
  const line = (t: string) => (/^L[0-9]+$/.test(t) && Number(t.slice(1)) >= 1 ? Number(t.slice(1)) : null)
  const dash = f.indexOf('-')
  const a = line(dash < 0 ? f : f.slice(0, dash))
  const b = dash < 0 ? a : line(f.slice(dash + 1))
  if (a == null || b == null) return bad
  if (b < a) return `L${b} is before L${a}`
  return [a, b]
}

/** Parse one anchor string; the `string` alternative is the message a located error carries. */
export function parseAnchor(s: string): Anchor | string {
  if (s.startsWith('block:')) {
    const id = s.slice('block:'.length)
    return id === '' || /\s/.test(id) ? 'block:<id> needs a non-blank id' : { kind: 'block', id }
  }
  const hash = s.indexOf('#')
  const path = hash < 0 ? s : s.slice(0, hash)
  const bad = checkPath(path)
  if (bad) return bad
  if (hash < 0) return { kind: 'code', path }
  const lines = parseLines(s.slice(hash + 1))
  return typeof lines === 'string' ? lines : { kind: 'code', path, lines }
}
