import { describe, expect, it } from 'vitest'
import type { DocMeta } from '@/rpc/bindings/DocMeta'
import type { DocumentChanged } from '@/rpc/bindings/DocumentChanged'
import { titleAfter } from './title'

const changed = (over: Partial<DocumentChanged>): DocumentChanged => ({
  type: 'DocumentChanged',
  doc: 'n1',
  kind: 'note',
  title: 'Renamed',
  updated_ms: 1n,
  deleted: false,
  ...over,
})

describe('titleAfter', () => {
  it('takes a rename of this document and updates meta', () => {
    const meta = { title: 'Old' } as DocMeta
    const p = { id: 'n1', meta }
    expect(titleAfter(p, changed({}))).toBe('Renamed')
    expect(meta.title).toBe('Renamed')
  })

  it('ignores other documents and trash moves', () => {
    const meta = { title: 'Old' } as DocMeta
    expect(titleAfter({ id: 'n1', meta }, changed({ doc: 'n2' }))).toBeNull()
    expect(titleAfter({ id: 'n1', meta }, changed({ deleted: true }))).toBeNull()
    expect(meta.title).toBe('Old')
  })

  it('works before doc_open delivered meta', () => {
    expect(titleAfter({ id: 'n1', meta: null }, changed({}))).toBe('Renamed')
  })
})
