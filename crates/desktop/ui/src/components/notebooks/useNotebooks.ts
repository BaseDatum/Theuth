import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { toast } from 'sonner'
import { notebookKeys, useNotebooksApi } from '@/lib/notebooks-rpc'

/** `notebook_list(archived)` (144 §12.3), live notebooks by default. */
export function useNotebooks(archived = false) {
  const nb = useNotebooksApi()
  return useQuery({ queryKey: notebookKeys.list(archived), queryFn: () => nb.notebook_list(archived) })
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
