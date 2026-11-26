import { AtSign, Contact, Globe, LayoutList, MessagesSquare, NotebookText } from 'lucide-react'
import { ContactsTab } from '@/components/settings/Contacts'
import { DomainsTab, SharingTab } from '@/components/settings/Sharing'
import { SharedChatsTab, SharedNotesTab, SharingOverviewTab } from '@/components/settings/SharingHub'
import type { SettingsCategory } from '@/components/settings/types'

// 213 §2: Settings › Sharing is where the things being shared are managed; Network keeps the switches and access.
// 210 §7.4 item 4: on every client (iOS and web get it from the same bundle, item 10). Each tab hides on an engine
// without its methods; Overview hides on one without `sharing_list`, so Contacts leads there.
// 191: a phone or web client with no local node sees Contacts (the mesh's) and adds them through the engine it controls.
export const sharing: SettingsCategory = {
  id: 'sharing',
  label: 'Sharing',
  icon: Globe,
  tabs: [
    { id: 'shared', label: 'Overview', icon: LayoutList, Component: SharingOverviewTab, required: { methods: ['sharing_list'] } },
    { required: {"methods":["a2a_export_put","a2a_export_remove","a2a_exports","agents_list","contact_answer","contact_first_answer","contact_first_waiting","contact_list","contact_nearby","contact_pair","contact_remove","contact_update","contact_window","list_sessions","mesh_status"],"topics":[]}, id: 'contacts', label: 'Contacts', icon: Contact, Component: ContactsTab },
    { id: 'shared-chats', label: 'Chats and agents', icon: MessagesSquare, Component: SharedChatsTab, required: { methods: ['a2a_exports', 'a2a_export_remove', 'contact_list', 'contact_update'] } },
    { id: 'shared-notes', label: 'Notes and notebooks', icon: NotebookText, Component: SharedNotesTab, required: { methods: ['sharing_list', 'sharing_revoke'] } },
    { id: 'sharing', label: 'Public shares', icon: Globe, Component: SharingTab, required: { methods: ['share_settings_get', 'share_list'] } },
    { id: 'domains', label: 'Domains', icon: AtSign, Component: DomainsTab, required: { methods: ['share_settings_get', 'share_list'] } },
  ],
}
