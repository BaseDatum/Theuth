import { withCapability } from '@/rpc/capabilities'
import { requirements } from '@/components/vm/requirements'
import { createFileRoute } from '@tanstack/react-router'
import { VmDesktop } from '@/components/vm/VmDesktop'

/** One sandbox VM's desktop, full window (`lib/popout.ts` kind `vm`, 79 §6.2). Under the `Popped` root when opened with `?window=`. */
export const Route = createFileRoute('/vm/$name')({
  component: withCapability(() => {
    const { name } = Route.useParams()
    return <VmDesktop name={name} />
  }, requirements),
})
