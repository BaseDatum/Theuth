import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import { expect, test } from 'vitest'
import { cellOf, digest, parseDefaults } from './grammar'

// 171 §3.1: the fixtures `surface::cell` is tested against (`crates/surface/tests/cells.rs`) — one file, two parsers.
const FIXTURES = fileURLToPath(new URL('../../../../../surface/tests/cells.json', import.meta.url))
type Fixture = { name: string; lang: string; body: string; cell: unknown; digest: string | null }
const doc = JSON.parse(readFileSync(FIXTURES, 'utf8')) as { cells: Fixture[]; defaults: { name: string; body: string; defaults: unknown }[] }

test('every cells.json fixture parses exactly as surface::cell::parse', () => {
  for (const f of doc.cells) expect(cellOf(f.lang, f.body), f.name).toEqual(f.cell)
})

test('every cells.json digest matches surface::cell::digest', async () => {
  for (const f of doc.cells) if (f.digest) expect(await digest(f.body), f.name).toBe(f.digest)
})

test('every note-defaults fixture matches surface::cell::parse_defaults', () => {
  for (const f of doc.defaults) expect(parseDefaults(f.body), f.name).toEqual(f.defaults)
})

test('digest normalises CRLF and trims exactly one final newline (§3.4)', async () => {
  const base = await digest('#| cwd: ~/x\necho hi')
  expect(await digest('#| cwd: ~/x\r\necho hi\r\n')).toBe(base)
  expect(await digest('#| cwd: ~/x\necho hi\n\n')).not.toBe(base)
  expect(base).toMatch(/^[0-9a-f]{64}$/)
})
