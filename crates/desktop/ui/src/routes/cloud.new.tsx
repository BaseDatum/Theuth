import { createFileRoute, useRouter } from '@tanstack/react-router'
import { CloudNodeWizard, type WizardEntry } from '@/components/cloud/CloudNodeWizard'
import { useIsMobile } from '@/hooks/use-mobile'

type Search = { entry?: WizardEntry; node?: string }
const ENTRIES: readonly WizardEntry[] = ['first-run', 'add-to-mesh', 'desktop-dialog']

/** 224: the cloud-node wizard as a page — the phone's first run and its "Add a cloud node" (full screen, over the shell). */
export const Route = createFileRoute('/cloud/new')({
  validateSearch: (s: Record<string, unknown>): Search => ({
    ...(ENTRIES.includes(s.entry as WizardEntry) ? { entry: s.entry as WizardEntry } : {}),
    ...(typeof s.node === 'string' && s.node.length > 0 ? { node: s.node } : {}),
  }),
  component: CloudNew,
})

function CloudNew() {
  const { entry, node } = Route.useSearch()
  const mobile = useIsMobile()
  const router = useRouter()
  const leave = () => (router.history.canGoBack() ? router.history.back() : void router.navigate({ to: '/' }))
  const e = entry ?? (mobile ? 'add-to-mesh' : 'desktop-dialog')
  const initial = node ? { step: 'progress' as const, node } : undefined
  // `onDone` with no node: the first run's "Just this iPhone"; with one, the wizard navigates on by itself.
  const done = (n: unknown) => !n && leave()
  if (e === 'desktop-dialog') return <CloudNodeWizard key={node ?? 'new'} entry={e} initial={initial} onClose={leave} onDone={done} />
  return (
    <div className="fixed inset-0 z-40 overflow-y-auto bg-background">
      <CloudNodeWizard key={node ?? 'new'} entry={e} initial={initial} onClose={leave} onDone={done} />
    </div>
  )
}
