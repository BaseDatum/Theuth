// 191 D8 visual check, dev only: `bunx vite` then open `/src/dev/contacts-fixture.html` (`?theme=dark`; `?unpair`
// shows the unpair dialog). The real Sharing › Contacts against `dev/local-fixture.ts`'s contacts (turned on here), no
// engine: every other call fails.
import '@/styles.css'
import { QueryClientProvider } from '@tanstack/react-query'
import { RouterProvider, createMemoryHistory, createRootRoute, createRouter } from '@tanstack/react-router'
import { StrictMode } from 'react'
import { createRoot } from 'react-dom/client'
import { ContactsTab } from '@/components/settings/Contacts'
import { UnpairBody } from '@/components/settings/UnpairDialog'
import { Dialog, DialogContent } from '@/components/ui/dialog'
import { TooltipProvider } from '@/components/ui/tooltip'
import type { Api } from '@/rpc/bindings/client'
import type { RpcClient } from '@/rpc/client'
import { active } from '@/store/engine-scope'

localStorage.setItem('local-fixture', '1')
const query = new URLSearchParams(location.search)
document.documentElement.classList.toggle('dark', query.get('theme') === 'dark')

const api = new Proxy({} as Api, {
  get: (_t, name: string) => async () => {
    throw new Error(`fixture: ${name} not answered`)
  },
})
active().connection.setState({ api, client: {} as RpcClient, info: null })

function Fixture() {
  return (
    <TooltipProvider>
      <div className="h-dvh overflow-y-auto">
        <div className="mx-auto flex max-w-[768px] flex-col gap-6 px-4 py-8">
          <ContactsTab focus={null} />
        </div>
      </div>
      {query.has('unpair') && (
        <Dialog open>
          <DialogContent className="max-h-[90dvh] overflow-y-auto sm:max-w-lg" aria-describedby={undefined}>
            <UnpairBody
              name="old-laptop"
              preview={{ id: 'old-laptop', name: 'old-laptop', connected: false, last_seen_ms: Date.now() - 3_600_000, secrets: ['ANTHROPIC_API_KEY'], sign_ins: [], offline: [], old_build: [], devices: 0 }}
              previewError={null}
              onRetryPreview={() => undefined}
              previewRetrying={false}
              previewFailedTwice={false}
              nameOf={(id) => id}
              reset={false}
              onReset={() => undefined}
              error={null}
              busy={false}
              onCancel={() => undefined}
              onConfirm={() => undefined}
              contacts={[{ name: 'bob' }, { name: 'carol' }]}
            />
          </DialogContent>
        </Dialog>
      )}
    </TooltipProvider>
  )
}

const router = createRouter({ routeTree: createRootRoute({ component: Fixture }), history: createMemoryHistory() })

createRoot(document.getElementById('root') as HTMLElement).render(
  <StrictMode>
    <QueryClientProvider client={active().qc}>
      <RouterProvider router={router} />
    </QueryClientProvider>
  </StrictMode>,
)
