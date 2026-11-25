import { useQuery, useQueryClient } from '@tanstack/react-query'
import { useCallback, useEffect } from 'react'
import { useQuerySupport } from '@/rpc/capabilities'
import type { DocKind } from '@/rpc/bindings/method.agent.output'
import { useApi } from '@/lib/rpc'
import { useConnection } from '@/store/connection'
import { on } from '@/store/sessions'
import { WHITEBOARD_KIND, wbRpc } from './wire'

export const whiteboardsKey = ['whiteboards'] as const

/** 218: every whiteboard, newest update first; `document.changed@1` invalidates it. */
export function useWhiteboards(limit = 500) {
  const canQuery = useQuerySupport(['doc_list'])
  const api = useApi()
  const qc = useQueryClient()
  useEffect(() => on('document.changed@1', 'DocumentChanged', () => void qc.invalidateQueries({ queryKey: whiteboardsKey })), [qc])
  return useQuery({
    enabled: canQuery,
    queryKey: [...whiteboardsKey, limit],
    queryFn: async () =>
      (await api.doc_list(WHITEBOARD_KIND as DocKind /* 218-TEMP(wire) */, limit)).sort((a, b) => Number(b.updated_ms - a.updated_ms)),
  })
}

/** 218: creates a whiteboard (`whiteboard_create`) and resolves to its doc id; the list refetches. */
export function useCreateWhiteboard(): (title?: string) => Promise<string> {
  const client = useConnection((s) => s.client)
  const qc = useQueryClient()
  return useCallback(
    async (title?: string) => {
      if (!client) throw new Error('not connected')
      const { doc } = await wbRpc(client).create(title ?? null)
      void qc.invalidateQueries({ queryKey: whiteboardsKey })
      return doc
    },
    [client, qc],
  )
}
