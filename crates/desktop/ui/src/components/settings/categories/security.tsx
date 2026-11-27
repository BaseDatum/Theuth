import { Lock, ShieldCheck } from 'lucide-react'
import type { SettingsCategory } from '@/components/settings/types'
import { vaultRequirements } from '@/components/vault/api'
import { VaultSettings } from '@/components/vault/VaultSettings'

// 225: the sealed vault: each machine's unlock methods and master password, and the machines this device unlocks.
export const security: SettingsCategory = {
  id: 'security',
  label: 'Security',
  icon: ShieldCheck,
  tabs: [{ required: vaultRequirements, id: 'vault', label: 'Vault', icon: Lock, Component: VaultSettings }],
}
