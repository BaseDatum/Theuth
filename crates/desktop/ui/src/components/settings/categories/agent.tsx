import { Bot, Eye, FlaskConical, Infinity as InfinityIcon, RefreshCw, ShieldCheck } from 'lucide-react'
import { Advisor } from '@/components/settings/Advisor'
import { Agents } from '@/components/settings/Agents'
import { Approvals } from '@/components/settings/Approvals'
import { Autoresearch } from '@/components/settings/Autoresearch'
import { Loop } from '@/components/settings/Loop'
import { Memory } from '@/components/settings/Memory'
import type { SettingsCategory } from '@/components/settings/types'

// 188: tab ids, labels, icons and order are frozen by C0 (docs/design/188-settings-full-page.md, Contract).
export const agent: SettingsCategory = {
  id: 'agent',
  label: 'Agent',
  icon: Bot,
  tabs: [
    { id: 'loop', label: 'Loop', icon: RefreshCw, Component: Loop },
    { id: 'memory', label: 'Memory', icon: InfinityIcon, Component: Memory },
    { id: 'approvals', label: 'Approvals', icon: ShieldCheck, Component: Approvals },
    { id: 'advisor', label: 'Advisor', icon: Eye, Component: Advisor },
    { id: 'autoresearch', label: 'Autoresearch', icon: FlaskConical, Component: Autoresearch },
    { id: 'harnesses', label: 'Harnesses', icon: Bot, Component: Agents },
  ],
}
