import { Box, MessagesSquare, Monitor, Network as NetworkIcon, Share2, Smartphone } from 'lucide-react'
import { MessagingTab } from '@/components/settings/Communication'
import { DevicesTab } from '@/components/settings/Link'
import { MachineTab, MachinesTab } from '@/components/settings/Network'
import { SandboxVms } from '@/components/settings/SandboxVms'
import type { SettingsCategory } from '@/components/settings/types'
import { hasLocal } from '@/link/engines'

// 188: a phone or web client with no local node reaches meshes as a device, so it sees Devices. 213: Contacts moved to
// Settings › Sharing (what is shared); Network keeps the switches and access.
export const network: SettingsCategory = {
  id: 'network',
  label: 'Network',
  icon: Share2,
  tabs: [
    { required: {"methods":["config_flag_get","config_flag_set","mesh_rename","mesh_set_hub","mesh_set_nearby","mesh_status","sessions_status","vault_rotate","vault_rotation"],"topics":[]}, id: 'machine', label: 'This machine', icon: Monitor, Component: MachineTab, visible: hasLocal },
    { required: {"methods":["add_node","contact_list","list_nodes","mesh_add","mesh_confirm_peer","mesh_nearby","mesh_nearby_watch","mesh_pair_answer","mesh_pair_nearby","mesh_pairing_string","mesh_remove","mesh_rename","mesh_set_messaging","mesh_status","mesh_unpair_preview","remove_node","rename_node","vault_rotate","vault_rotation"],"topics":["nodes.events@1"]}, id: 'machines', label: 'Machines', icon: NetworkIcon, Component: MachinesTab, visible: hasLocal },
    { required: {"methods":["device_pair_start","device_rename","device_revoke","devices_list","link_enable","link_rename","link_revoke_device","link_status","mesh_status"],"topics":["link.events@1"]}, id: 'devices', label: 'Devices', icon: Smartphone, Component: DevicesTab },
    { required: {"methods":["a2a_status","config_flag_get","config_flag_set","contact_first_answer","contact_first_waiting","contact_list","get_communication","reload_a2a","set_communication"],"topics":[]}, id: 'messaging', label: 'Messaging', icon: MessagesSquare, Component: MessagingTab, visible: hasLocal },
    { required: {"methods":["handoffs_list","list_sessions","mesh_status","sandbox_create","sandbox_destroy","sandbox_install_runtime","sandbox_pause","sandbox_preflight","sandbox_resize","sandbox_restore","sandbox_resume","sandbox_retry","sandbox_set_shares","sandbox_snapshot","sandbox_start","sandbox_stop","sandbox_terminal","sandboxes_list"],"topics":[]}, id: 'vms', label: 'Sandbox VMs', icon: Box, Component: SandboxVms, visible: hasLocal },
  ],
}
