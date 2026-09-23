import { expect, test } from 'vitest'
import type { ArchiveEntry } from '@/rpc/bindings/ArchiveEntry'
import { build } from './ArchiveView'

const e = (path: string, size = 0, is_dir = false): ArchiveEntry => ({ path, size, compressed: null, mtime: null, is_dir, mode: null })

test('paths become a tree with implied folders and summed sizes', () => {
  // `src/` is implied by its members; `docs/` is an explicit directory entry; sizes roll up.
  const root = build([e('src/main.rs', 12), e('src/lib/mod.rs', 8), e('docs/', 0, true), e('README', 2)])
  expect([...root.children.keys()]).toEqual(['src', 'docs', 'README'])
  const src = root.children.get('src')!
  expect(src.dir).toBe(true)
  expect(src.size).toBe(20)
  expect(src.children.get('lib')!.children.get('mod.rs')!.size).toBe(8)
  expect(root.children.get('docs')!.dir).toBe(true)
  expect(root.children.get('README')!.dir).toBe(false)
  expect(root.size).toBe(22)
})
