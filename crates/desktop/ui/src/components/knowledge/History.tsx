import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { toast } from 'sonner'
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from '@/components/ui/alert-dialog'
import { Button } from '@/components/ui/button'
import { Sheet, SheetContent, SheetDescription, SheetHeader, SheetTitle } from '@/components/ui/sheet'
import { Skeleton } from '@/components/ui/skeleton'
import { queryKeys, useApi } from '@/lib/rpc'
import type { Change } from '@/rpc/bindings/Change'

type Props = { id: string; open: boolean; onOpenChange: (open: boolean) => void }

/** The concept's last 20 commits with the latest diff; Restore rewrites the file from a commit. */
export function History({ id, open, onOpenChange }: Props) {
  const api = useApi()
  const qc = useQueryClient()
  const [restoring, setRestoring] = useState<Change | null>(null)
  const history = useQuery({
    queryKey: queryKeys.knowledgeHistory(id),
    queryFn: () => api.knowledge_history(id, 20),
    enabled: open,
  })
  const restore = useMutation({
    mutationFn: (commit: string) => api.knowledge_restore(id, commit),
    onSuccess: (c) => {
      qc.invalidateQueries({ queryKey: queryKeys.knowledgeDoc(id) })
      qc.invalidateQueries({ queryKey: queryKeys.knowledgeList })
      qc.invalidateQueries({ queryKey: queryKeys.knowledgeHistory(id) })
      toast(`Restored ${id} from ${c.commit}`)
    },
    onError: (e) => toast.error(e.message),
    onSettled: () => setRestoring(null),
  })

  return (
    <Sheet open={open} onOpenChange={onOpenChange}>
      <SheetContent className="flex w-[480px] flex-col gap-0 sm:max-w-[480px]">
        <SheetHeader>
          <SheetTitle>History</SheetTitle>
          <SheetDescription className="truncate">{id}</SheetDescription>
        </SheetHeader>
        <div className="min-h-0 flex-1 overflow-y-auto px-4 pb-4">
          {history.isPending ? (
            <div className="space-y-2">
              <Skeleton className="h-9" />
              <Skeleton className="h-9" />
            </div>
          ) : history.isError ? (
            <p className="text-sm text-danger">{history.error.message}</p>
          ) : (
            <>
              <ul className="divide-y divide-border-subtle">
                {history.data.changes.map((c) => (
                  <li key={c.commit} className="flex items-center gap-3 py-2 text-sm">
                    <code className="shrink-0 font-mono text-xs text-fg-tertiary">{c.commit}</code>
                    <span className="shrink-0 text-xs text-fg-tertiary tabular-nums">{c.time}</span>
                    <span className="min-w-0 flex-1 truncate">{c.message}</span>
                    <Button variant="ghost" size="xs" onClick={() => setRestoring(c)}>
                      Restore
                    </Button>
                  </li>
                ))}
              </ul>
              {history.data.diff && (
                <pre className="mt-4 overflow-x-auto rounded-2xl bg-layer-2 p-4 font-mono text-xs leading-5 whitespace-pre">
                  {history.data.diff}
                </pre>
              )}
            </>
          )}
        </div>
      </SheetContent>
      <AlertDialog open={restoring !== null} onOpenChange={(o) => !o && setRestoring(null)}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>Restore {restoring?.commit}?</AlertDialogTitle>
            <AlertDialogDescription>
              {id} is rewritten as of {restoring?.time} ({restoring?.message}). The current version stays in history.
            </AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>Cancel</AlertDialogCancel>
            <AlertDialogAction disabled={restore.isPending} onClick={() => restoring && restore.mutate(restoring.commit)}>
              Restore
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
    </Sheet>
  )
}
