// 216 §12, contract §12.6: Settings › Plugins' "New plugin" — a name, the SDK language, and Raw (the no-SDK protocol
// template, py and sh only) → `plugin_new`. Nothing runs: the new folder files its install card, which the notice names.
import { Plus } from 'lucide-react'
import { useState } from 'react'
import { useAction } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { Popover, PopoverContent, PopoverTrigger } from '@/components/ui/popover'
import { Switch } from '@/components/ui/switch'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import { notify as toast } from '@/lib/notify'
import { queryKeys, useApi } from '@/lib/rpc'
import type { Lang } from '@/rpc/bindings/method.plugins.output'

/** The SDK languages in `t plugin new --lang` order, with their names. */
export const LANGS: readonly (readonly [Lang, string])[] = [
  ['py', 'Python'],
  ['js', 'JavaScript'],
  ['sh', 'Shell'],
  ['go', 'Go'],
  ['rb', 'Ruby'],
  ['rs', 'Rust'],
]

/** Only these have a raw (no-SDK) template. */
const RAW_LANGS: Partial<Record<Lang, true>> = { py: true, sh: true }

/** The host's name rule (`manifest::check_name`): a lowercase letter, then up to 31 of `a-z 0-9 -`. */
export const PLUGIN_NAME = /^[a-z][a-z0-9-]{0,31}$/

export function NewPluginPopover() {
  const api = useApi()
  const [open, setOpen] = useState(false)
  const [name, setName] = useState('')
  const [lang, setLang] = useState<Lang>('py')
  const [raw, setRaw] = useState(false)
  const rawOk = RAW_LANGS[lang] === true
  const create = useAction((v: { name: string; lang: Lang; raw: boolean }) => api.plugin_new(v.name, v.lang, v.raw), {
    keys: [queryKeys.plugins],
    onSuccess: (r) => {
      toast.success(`Created ${r.name}`, { description: r.next })
      setName('')
      setRaw(false)
      setOpen(false)
    },
  })
  const valid = PLUGIN_NAME.test(name)
  return (
    <Popover open={open} onOpenChange={setOpen}>
      <PopoverTrigger asChild>
        <Button variant="ghost" size="sm">
          <Plus />
          New plugin
        </Button>
      </PopoverTrigger>
      <PopoverContent align="end" className="w-[min(380px,calc(100vw-2rem))] space-y-3">
        <p className="text-sm text-fg-secondary">A new folder under plugins/ from the SDK template. Nothing runs until you approve it.</p>
        <form
          className="space-y-3"
          onSubmit={(e) => {
            e.preventDefault()
            if (valid && !create.isPending) create.mutate({ name, lang, raw: rawOk && raw })
          }}
        >
          <Input value={name} onChange={(e) => setName(e.target.value.trim())} placeholder="my-plugin" aria-label="Plugin name" spellCheck={false} autoCapitalize="off" className="font-mono" />
          {name !== '' && !valid && <p className="text-[13px] text-destructive">A lowercase letter, then up to 31 of a–z, 0–9 and -.</p>}
          <ToggleGroup type="single" variant="outline" size="sm" value={lang} onValueChange={(v) => v && setLang(v as Lang)} aria-label="Language" className="flex-wrap">
            {LANGS.map(([id, label]) => (
              <ToggleGroupItem key={id} value={id} title={label}>
                {id}
              </ToggleGroupItem>
            ))}
          </ToggleGroup>
          <label className="flex items-center justify-between gap-3 text-sm">
            <span className={rawOk ? undefined : 'text-fg-tertiary'}>
              Raw protocol, no SDK
              <span className="block text-[13px] text-fg-tertiary">Python and Shell only</span>
            </span>
            <Switch checked={rawOk && raw} disabled={!rawOk} onCheckedChange={setRaw} aria-label="Raw protocol, no SDK" />
          </label>
          <div className="flex justify-end">
            <Button size="sm" type="submit" disabled={!valid || create.isPending}>
              Create
            </Button>
          </div>
        </form>
      </PopoverContent>
    </Popover>
  )
}
