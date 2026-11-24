import { useQuerySupport } from '@/rpc/capabilities'
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { notify as toast } from '@/lib/notify'
import { notebookKeys, useNotebooksApi } from '@/lib/notebooks-rpc'

/** `notebook_list(archived)` (144 §12.3), live notebooks by default. `enabled: false` reads the cache only. */
export function useNotebooks(archived = false, enabled = true) {
  const canQuery1 = useQuerySupport(["notebook_list"])
  const nb = useNotebooksApi()
  return useQuery({ enabled: enabled && canQuery1, queryKey: notebookKeys.list(archived), queryFn: () => nb.notebook_list(archived) })
}

/** `notebook_create` → the new row; the caller navigates. */
export function useCreateNotebook(onCreated: (id: string) => void) {
  const nb = useNotebooksApi()
  const qc = useQueryClient()
  return useMutation({
    mutationFn: (title: string) => nb.notebook_create(title, null, null),
    onSuccess: (n) => {
      void qc.invalidateQueries({ queryKey: notebookKeys.all })
      onCreated(n.doc.id)
    },
    onError: (e) => toast.error(e.message),
  })
}
