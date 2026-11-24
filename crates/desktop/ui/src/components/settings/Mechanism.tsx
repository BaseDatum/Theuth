import { type ReactNode, useState } from 'react'
import { notify as toast } from '@/lib/notify'
import { shortSha } from '@/components/autoresearch/model'
import { KEY, useAutoresearchApi, useMechanism, useMechanismFiles } from '@/components/autoresearch/queries'
import { Empty, Row, Section, useAction } from '@/components/settings/shared'
import { AlertDialog, AlertDialogAction, AlertDialogCancel, AlertDialogContent, AlertDialogDescription, AlertDialogFooter, AlertDialogHeader, AlertDialogTitle, AlertDialogTrigger } from '@/components/ui/alert-dialog'
import { Badge } from '@/components/ui/badge'
import { Button } from '@/components/ui/button'
import type { Api } from '@/rpc/bindings/client'
import type { MechanismInfo } from "@/rpc/bindings/method.autoresearch.output"

/** `autoresearch.mechanism {op: adopt, version}`; `0` reverts to the compiled defaults. The press adopts, never the model (159 §15). */
export const adoptMechanism = (api: Pick<Api, 'autoresearch.mechanism'>, version: number): Promise<MechanismInfo> => api['autoresearch.mechanism']({ op: 'adopt', version })

/** The toast after an adopt: what is active now. */
export const adoptedText = (info: Pick<MechanismInfo, 'active'>): string => (info.active === null ? 'Reverted to the compiled defaults' : `Adopted mechanism v${info.active}`)

/**
 * Settings › Autoresearch › Mechanism (159 §12, W7 H9): the active version with Revert, every registered version with
 * Show (its files, inline) and Adopt behind a confirm. New studies pin the active version; running studies keep theirs.
 */
export function Mechanism() {
  const api = useAutoresearchApi()
  const list = useMechanism()
  const [shown, setShown] = useState<number | null>(null)
  const files = useMechanismFiles(shown)
  const adopt = useAction((version: number) => adoptMechanism(api, version), { keys: [[...KEY, 'mechanism']], onSuccess: (info) => toast.success(adoptedText(info)) })
  const m = list.data
  const active = m?.active ?? null
  const busy = !m || adopt.isPending
  return (
    <Section title="Mechanism">
      {list.isError ? <Empty>{list.error.message}</Empty> : null}
      <Row label="Active version" description="The protocol, fire and advisor texts and the parameters new studies pin; running studies keep theirs. Only a press here adopts a version — a meta study proposes, never adopts.">
        <span className="text-sm text-fg-secondary">{m ? (active === null ? 'compiled defaults' : `v${active}`) : '…'}</span>
        <Confirm title="Revert to the compiled defaults?" description="New studies use the built-in mechanism again. The versions stay listed; running studies are not touched." action="Revert" onConfirm={() => adopt.mutate(0)}>
          <Button variant="outline" size="sm" disabled={busy || active === null}>
            Revert
          </Button>
        </Confirm>
      </Row>
      {m?.versions.length === 0 && <Empty>No versions yet — a finished meta study registers its candidate here.</Empty>}
      {m?.versions.map((v) => (
        <div key={v.version}>
          <Row
            label={
              <span className="flex items-center gap-2">
                v{v.version}
                {v.version === active && (
                  <Badge variant="secondary" className="text-[10px]">
                    active
                  </Badge>
                )}
              </span>
            }
            description={`${shortSha(v.sha)} · ${new Date(v.at).toLocaleString()} · ${v.summary}`}
          >
            <Button variant="ghost" size="sm" aria-expanded={shown === v.version} onClick={() => setShown(shown === v.version ? null : v.version)}>
              {shown === v.version ? 'Hide' : 'Show'}
            </Button>
            <Confirm title={`Adopt mechanism v${v.version}?`} description={`${v.summary}. New studies pin it from now on; running studies keep the one they started with.`} action="Adopt" onConfirm={() => adopt.mutate(v.version)}>
              <Button variant="outline" size="sm" disabled={busy || v.version === active}>
                Adopt
              </Button>
            </Confirm>
          </Row>
          {shown === v.version && (
            <div className="flex flex-col gap-3 pb-4">
              {files.isError && <Empty>{files.error.message}</Empty>}
              {files.isPending && <Empty>…</Empty>}
              {files.data &&
                Object.entries(files.data.files).map(([name, text]) => (
                  <div key={name}>
                    <div className="mb-1 font-mono text-[12px] text-fg-tertiary">{name}</div>
                    <pre className="max-h-80 overflow-auto rounded-md bg-layer-2 p-3 font-mono text-xs leading-5 whitespace-pre-wrap break-all text-fg-secondary">{text}</pre>
                  </div>
                ))}
            </div>
          )}
        </div>
      ))}
    </Section>
  )
}

/** The confirm every adopt goes through: the trigger is `children`, `action` the confirming word. */
function Confirm({ title, description, action, onConfirm, children }: { title: string; description: string; action: string; onConfirm: () => void; children: ReactNode }) {
  return (
    <AlertDialog>
      <AlertDialogTrigger asChild>{children}</AlertDialogTrigger>
      <AlertDialogContent size="sm">
        <AlertDialogHeader>
          <AlertDialogTitle>{title}</AlertDialogTitle>
          <AlertDialogDescription>{description}</AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel>Cancel</AlertDialogCancel>
          <AlertDialogAction onClick={onConfirm}>{action}</AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  )
}
