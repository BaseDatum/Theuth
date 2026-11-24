// 210 FIX-A M9: a password share's generated password, shown to the owner once with a copy button. It comes from
// `share_password {id}` (owner-only RPC, engine memory) or the RPC `Published.password`; never from the tool output
// (the model must not see it), never logged, never put in a store.

import { Copy, KeyRound } from 'lucide-react'
import { useEffect, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { takePassword } from '@/components/public-share/actions'
import { Button } from '@/components/ui/button'

/** The password of share `id` the first time this webview asks (`null` id = nothing to ask). */
export function useSharePassword(id: string | null, known?: string | null): [string | null, () => void] {
  const [password, setPassword] = useState<string | null>(null)
  useEffect(() => {
    if (!id) return
    let live = true
    void takePassword(id, known).then((p) => {
      if (live && p) setPassword(p)
    })
    return () => {
      live = false
    }
  }, [id, known])
  return [password, () => setPassword(null)]
}

export function PasswordOnce({ password, onDone }: { password: string; onDone: () => void }) {
  return (
    <div data-share-password="" className="mt-2 rounded-lg border border-border-subtle bg-bg-secondary px-3 py-2 text-xs">
      <div className="flex items-center gap-2">
        <KeyRound className="size-3.5 shrink-0" />
        <span className="text-fg-secondary">Password</span>
        <code className="min-w-0 flex-1 truncate font-mono text-sm select-all">{password}</code>
        <Button variant="ghost" size="icon" className="size-6" aria-label="Copy password" onClick={() => void navigator.clipboard.writeText(password).then(() => toast.success('Password copied'))}>
          <Copy className="size-3.5" />
        </Button>
        <Button size="sm" variant="outline" className="h-6" onClick={onDone}>
          Done
        </Button>
      </div>
      <p className="mt-1 text-fg-secondary">Shown once: copy it now and give it to the people you share with. The agent does not see it.</p>
    </div>
  )
}
