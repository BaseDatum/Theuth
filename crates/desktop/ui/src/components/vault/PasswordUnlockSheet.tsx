import { useQueryClient } from '@tanstack/react-query'
import { useState } from 'react'
import { errorText } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogFooter, DialogHeader, DialogTitle } from '@/components/ui/dialog'
import { Input } from '@/components/ui/input'
import { notify } from '@/lib/notify'
import { isWrongPassword, useVaultApi, vaultKeys } from './api'

/**
 * 225: unseal one sealed engine with its master password (`vault_unseal(engine, password)`). The password lives in this
 * component's state only and is cleared after every attempt, right or wrong; a wrong one says so inline.
 */
export function PasswordUnlockSheet({ engine, name, open, onOpenChange }: { engine: string | null; name: string; open: boolean; onOpenChange: (open: boolean) => void }) {
  const api = useVaultApi()
  const qc = useQueryClient()
  const [password, setPassword] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const close = (next: boolean) => {
    if (!next) {
      setPassword('')
      setError(null)
    }
    onOpenChange(next)
  }

  const submit = async () => {
    setBusy(true)
    setError(null)
    try {
      const status = await api.unseal(engine, password)
      qc.setQueryData(vaultKeys.status(engine), status)
      notify.success(`${name}'s vault is unlocked`)
      close(false)
    } catch (e) {
      setError(isWrongPassword(e) ? 'Wrong password' : errorText(e))
    } finally {
      setPassword('')
      setBusy(false)
    }
  }

  return (
    <Dialog open={open} onOpenChange={close}>
      <DialogContent>
        <DialogHeader>
          <DialogTitle>Unlock {name}</DialogTitle>
          <DialogDescription>
            Your master password is sent end-to-end encrypted to {name}, and the vault key is derived there. It is never stored.
          </DialogDescription>
        </DialogHeader>
        <form
          className="flex flex-col gap-2"
          onSubmit={(e) => {
            e.preventDefault()
            if (password) void submit()
          }}
        >
          <Input
            type="password"
            autoComplete="off"
            autoFocus
            aria-label={`master password for ${name}`}
            placeholder="Master password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            disabled={busy}
          />
          {error && (
            <div role="alert" className="text-xs text-danger">
              {error}
            </div>
          )}
          <DialogFooter>
            <Button type="button" variant="outline" disabled={busy} onClick={() => close(false)}>
              Cancel
            </Button>
            <Button type="submit" disabled={busy || !password}>
              Unlock
            </Button>
          </DialogFooter>
        </form>
      </DialogContent>
    </Dialog>
  )
}
