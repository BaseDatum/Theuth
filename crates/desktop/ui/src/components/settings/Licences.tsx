import { invoke } from '@tauri-apps/api/core'
import { useState } from 'react'
import { Row } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Dialog, DialogContent, DialogDescription, DialogHeader, DialogTitle } from '@/components/ui/dialog'

/** The legal files the desktop bundle carries in its resources (221 D22/B10), read by the shell's `legal_text`. */
const FILES = [
  ['LICENSE', 'Licence'],
  ['NOTICE', 'Notice'],
  ['THIRD-PARTY-NOTICES', 'Third-party notices'],
] as const

type File = (typeof FILES)[number][0]

/** Settings › App: theuth's licence (GPL-3.0-or-later, D26), the NOTICE for the linked Bitwarden SDK and the third-party notices. */
export function Licences() {
  const [open, setOpen] = useState<File | null>(null)
  const [text, setText] = useState<string | null>(null)
  const show = (name: File) => {
    setOpen(name)
    setText(null)
    invoke<string>('legal_text', { name }).then(setText, (e: unknown) => setText(`Not in this build: ${String(e)}`))
  }
  return (
    <>
      <Row label="Licences" description="theuth is free software under GPL-3.0-or-later; this app links Bitwarden's SDK under its GPL-3.0 option.">
        {FILES.map(([name, label]) => (
          <Button key={name} variant="outline" size="sm" onClick={() => show(name)}>
            {label}
          </Button>
        ))}
      </Row>
      <Dialog open={open !== null} onOpenChange={(o) => !o && setOpen(null)}>
        <DialogContent className="sm:max-w-3xl">
          <DialogHeader>
            <DialogTitle>{FILES.find(([name]) => name === open)?.[1]}</DialogTitle>
            <DialogDescription>{open}</DialogDescription>
          </DialogHeader>
          <pre className="max-h-[70vh] overflow-auto whitespace-pre-wrap font-mono text-xs text-fg-secondary">{text ?? 'Loading…'}</pre>
        </DialogContent>
      </Dialog>
    </>
  )
}
