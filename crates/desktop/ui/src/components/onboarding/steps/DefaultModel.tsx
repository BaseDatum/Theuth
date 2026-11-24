import { ArrowRight, Check, LoaderCircle } from 'lucide-react'
import { useState } from 'react'
import { useModels } from '@/components/agents/queries'
import { ModelCombobox } from '@/components/chat/ModelCombobox'
import { BUILTIN_LABEL } from '@/components/settings/providers'
import { errorText } from '@/components/settings/shared'
import { Button } from '@/components/ui/button'
import { useApi } from '@/lib/rpc'
import { useQuerySupport } from '@/rpc/capabilities'
import { useConnection } from '@/store/connection'
import { CARD, StepFrame } from './Frame'
import { contextLabel, recommendModels } from './content'

/** Step 3: three to five good defaults from what is connected (the provider just connected first), or any model. */
export function DefaultModel({ prefer, onBack, onNext }: { prefer: string | null; onBack(): void; onNext(): void }) {
  const api = useApi()
  const can = useQuerySupport(['set_model'])
  const models = useModels().data ?? []
  const current = useConnection((s) => s.info?.default_model)
  const recs = recommendModels(models, prefer, current)
  const [picked, setPicked] = useState<string | null>(null)
  const [state, setState] = useState<{ busy: boolean; error: string | null }>({ busy: false, error: null })
  const chosen = picked ?? (recs.some((m) => m.id === current) ? current : recs[0]?.id) ?? null
  const name = models.find((m) => m.id === chosen)?.name ?? chosen
  const save = async () => {
    if (!chosen || chosen === current || !can) return onNext()
    setState({ busy: true, error: null })
    try {
      // `session: null` + `manual` sets the default; `info` follows through `DefaultModelChanged`.
      await api.set_model(null, chosen, true)
      onNext()
    } catch (e) {
      setState({ busy: false, error: errorText(e) })
    }
  }
  return (
    <StepFrame
      eyebrow="Step 2 of 3"
      title="Pick your default model"
      description="Every new chat starts on it. You can switch any chat to another model at any time."
      back={
        <Button variant="ghost" onClick={onBack} className="text-fg-secondary">
          Back
        </Button>
      }
      primary={
        <Button size="lg" onClick={() => void save()} disabled={state.busy}>
          {state.busy && <LoaderCircle className="animate-spin motion-reduce:animate-none" />}
          {chosen ? `Use ${name}` : 'Continue'} <ArrowRight />
        </Button>
      }
    >
      {recs.length === 0 ? (
        <p className="max-w-2xl rounded-xl border border-border-subtle bg-layer-2 p-5 text-sm text-fg-secondary">
          No models yet. Connect one in the previous step, or continue and add one later in Settings › Models.
        </p>
      ) : (
        <div className="flex max-w-2xl flex-col gap-5">
          <div role="radiogroup" aria-label="Recommended models" className="grid gap-3 sm:grid-cols-2">
            {recs.map((m) => {
              const on = m.id === chosen
              return (
                <button
                  key={m.id}
                  type="button"
                  role="radio"
                  aria-checked={on}
                  onClick={() => setPicked(m.id)}
                  className={`${CARD} relative flex flex-col gap-1 p-4 ${on ? 'border-fg-accent bg-layer-3 shadow-[inset_0_0_0_1px_var(--fg-accent)]' : ''}`}
                >
                  <span className="pr-6 text-[15px] font-medium text-foreground">{m.name}</span>
                  <span className="text-[13px] text-fg-secondary">{[BUILTIN_LABEL[m.provider] ?? m.provider, contextLabel(m.context), m.local ? 'on this computer' : ''].filter(Boolean).join(' · ')}</span>
                  {m.id === current && <span className="text-[12px] text-fg-tertiary">Current default</span>}
                  {on && <Check className="absolute top-4 right-4 size-4 text-fg-accent" aria-hidden />}
                </button>
              )
            })}
          </div>
          <div className="flex flex-wrap items-center gap-3 text-sm text-fg-secondary">
            <span aria-hidden>Or choose from every model</span>
            <ModelCombobox aria-label="See all models" className="max-w-72" models={models} value={chosen} onChange={(id) => id && setPicked(id)} />
          </div>
          {state.error && (
            <p role="alert" className="text-[13px] text-danger">
              {state.error}
            </p>
          )}
        </div>
      )}
    </StepFrame>
  )
}
