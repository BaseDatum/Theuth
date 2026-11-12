import { useQuerySupport } from '@/rpc/capabilities'
import { useQuery } from '@tanstack/react-query'
import { queryKeys, useApi } from '@/lib/rpc'

/** The notes list, newest update first (59 §7.3); boards are notes too (71 §2.11). `DocumentChanged` invalidates it. */
export function useNotes() {
  const canQuery1 = useQuerySupport(["doc_list"])
  const api = useApi()
  return useQuery({ enabled: canQuery1,
    queryKey: queryKeys.notes,
    queryFn: async () => {
      const [notes, boards] = await Promise.all([api.doc_list('note', 500), api.doc_list('board', 500)])
      return [...notes, ...boards].sort((a, b) => Number(b.updated_ms - a.updated_ms))
    },
  })
}
