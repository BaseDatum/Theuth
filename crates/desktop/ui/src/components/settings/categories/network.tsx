import { Box, Contact, MessagesSquare, Monitor, Network as NetworkIcon, Share2, Smartphone } from 'lucide-react'
import { MessagingTab } from '@/components/settings/Communication'
import { ContactsTab } from '@/components/settings/Contacts'
import { DevicesTab } from '@/components/settings/Link'
import { MachineTab, MachinesTab } from '@/components/settings/Network'
import { SandboxVms } from '@/components/settings/SandboxVms'
import type { SettingsCategory } from '@/components/settings/types'
import { hasLocal } from '@/link/engines'

// 188: a phone or web client with no local node reaches meshes as a device, so it sees only Devices.
export const network: SettingsCategory = {
  id: 'network',
  label: 'Network',
  icon: Share2,
  tabs: [
    { id: 'machine', label: 'This machine', icon: Monitor, Component: MachineTab, visible: hasLocal },
    { id: 'machines', label: 'Machines', icon: NetworkIcon, Component: MachinesTab, visible: hasLocal },
    { id: 'devices', label: 'Devices', icon: Smartphone, Component: DevicesTab },
    { id: 'contacts', label: 'Contacts', icon: Contact, Component: ContactsTab, visible: hasLocal },
    { id: 'messaging', label: 'Messaging', icon: MessagesSquare, Component: MessagingTab, visible: hasLocal },
    { id: 'vms', label: 'Sandbox VMs', icon: Box, Component: SandboxVms, visible: hasLocal },
  ],
}
