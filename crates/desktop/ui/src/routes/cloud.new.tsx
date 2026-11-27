import { createFileRoute, useRouter } from '@tanstack/react-router'
import { CloudNodeWizard, type WizardEntry } from '@/components/cloud/CloudNodeWizard'
import { useIsMobile } from '@/hooks/use-mobile'

type Search = { entry?: WizardEntry }
const ENTRIES: readonly WizardEntry[] = ['first-run', 'add-to-mesh', 'desktop-dialog']

/** 224: the cloud-node wizard as a page — the phone's first run and its "Add a cloud node" (full screen, over the shell). */
export const Route = createFileRoute('/cloud/new')({
  validateSearch: (s: Record<string, unknown>): Search => (ENTRIES.includes(s.entry as WizardEntry) ? { entry: s.entry as WizardEntry } : {}),
  component: CloudNew,
})

function CloudNew() {
  const { entry } = Route.useSearch()
  const mobile = useIsMobile()
  const router = useRouter()
  const leave = () => (router.history.canGoBack() ? router.history.back() : void router.navigate({ to: '/' }))
  const e = entry ?? (mobile ? 'add-to-mesh' : 'desktop-dialog')
  // `onDone` with no node: the first run's "Just this iPhone"; with one, the wizard navigates on by itself.
  const done = (node: unknown) => !node && leave()
  if (e === 'desktop-dialog') return <CloudNodeWizard entry={e} onClose={leave} onDone={done} />
  return (
    <div className="fixed inset-0 z-40 overflow-y-auto bg-background">
      <CloudNodeWizard entry={e} onClose={leave} onDone={done} />
    </div>
  )
}
