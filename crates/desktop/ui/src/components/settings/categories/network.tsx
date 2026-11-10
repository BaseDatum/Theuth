import { Box, Contact, MessagesSquare, Monitor, Network as NetworkIcon, Share2, Smartphone } from 'lucide-react'
import { Communication } from '@/components/settings/Communication'
import { Contacts } from '@/components/settings/Contacts'
import { Link } from '@/components/settings/Link'
import { Network } from '@/components/settings/Network'
import { SandboxVms } from '@/components/settings/SandboxVms'
import type { SettingsCategory } from '@/components/settings/types'

// 188 C0: each tab renders one of today's components until this category's worker splits it
// (docs/design/188-settings-full-page.md, Contract). Only that worker edits this file.
export const network: SettingsCategory = {
  id: 'network',
  label: 'Network',
  icon: Share2,
  tabs: [
    { id: 'machine', label: 'This machine', icon: Monitor, Component: Network },
    { id: 'machines', label: 'Machines', icon: NetworkIcon, Component: Network },
    { id: 'devices', label: 'Devices', icon: Smartphone, Component: Link },
    { id: 'contacts', label: 'Contacts', icon: Contact, Component: () => <Contacts /> },
    { id: 'messaging', label: 'Messaging', icon: MessagesSquare, Component: () => <Communication session={null} focus={null} /> },
    { id: 'vms', label: 'Sandbox VMs', icon: Box, Component: SandboxVms },
  ],
}
