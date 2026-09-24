import { useQuery } from '@tanstack/react-query'
import { queryKeys, useApi } from '@/lib/rpc'

/** The notes list, newest update first (59 §7.3); `DocumentChanged` invalidates it. */
export function useNotes() {
  const api = useApi()
  return useQuery({ queryKey: queryKeys.notes, queryFn: () => api.doc_list('note', 500) })
}
