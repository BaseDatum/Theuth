import { SCOPE_LABEL, type ScopeNames, type SettingsNeed, type SettingsScope, notApplicableText, scopeTooltip } from '@/components/settings/scope'
import { cn } from '@/lib/utils'

/** Mineral project hues: muted on the light page, brighter in dark; the border a faint copy of the text. */
const TONE: Readonly<Record<SettingsScope, string>> = {
  machine: 'text-project-lapis border-project-lapis/35',
  everywhere: 'text-project-malachite border-project-malachite/35',
  hub: 'text-project-plum border-project-plum/35',
  device: 'text-project-sand border-project-sand/35',
}

/** The quiet scope tag of a nav row or a tab header (mock: `machine`, `everywhere`, `hub only`, `this device`). */
export function ScopeTag({ scope, names, className }: { scope: SettingsScope; names: ScopeNames; className?: string }) {
  return (
    <span
      data-scope={scope}
      title={scopeTooltip(scope, names)}
      className={cn('inline-flex shrink-0 items-center rounded-full border px-1.5 text-[10px] leading-4 font-normal whitespace-nowrap', TONE[scope], className)}
    >
      {SCOPE_LABEL[scope]}
    </span>
  )
}

/** A nav row's mark for a tab this engine cannot run (mock: `n/a`). */
export function NotHereTag({ missing, engine, className }: { missing: readonly SettingsNeed[]; engine: string | null; className?: string }) {
  return (
    <span data-not-here="" title={`Not on this machine. ${notApplicableText(missing, engine)}`} className={cn('shrink-0 text-[10px] leading-4 whitespace-nowrap text-fg-tertiary', className)}>
      n/a
    </span>
  )
}

/** The top of a tab's body: its scope tag, and why it does nothing on this engine when it does not apply. */
export function ScopeHeader({ scope, names, missing }: { scope: SettingsScope; names: ScopeNames; missing: readonly SettingsNeed[] | null }) {
  return (
    <div className="mb-3 flex flex-col gap-2">
      <ScopeTag scope={scope} names={names} className="self-start" />
      {missing && (
        <p role="note" className="rounded-[10px] border border-border-subtle bg-layer-2 px-3 py-2 text-sm text-fg-secondary">
          <span className="font-medium text-foreground">Not on this machine.</span> {notApplicableText(missing, names.engine)}
        </p>
      )}
    </div>
  )
}
