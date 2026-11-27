import { Cloud, Cpu, Wand2 } from 'lucide-react'
import { Helpers } from '@/components/settings/Helpers'
import { ModelDefaults } from '@/components/settings/ModelDefaults'
import { ModelProviders } from '@/components/settings/ModelProviders'
import type { SettingsCategory } from '@/components/settings/types'

// 188: tab ids, labels, icons and order are frozen by C0 (docs/design/188-settings-full-page.md, Contract).
export const models: SettingsCategory = { 
  id: 'models',
  label: 'Models',
  icon: Cpu,
  tabs: [
    { required: {"methods":["list_models","set_effort","set_model","set_model_favorite","set_subagent_effort","set_subagent_model","subagent_effort","subagent_model"],"topics":[]}, id: 'models', scope: 'machine', copyable: true, label: 'Defaults', icon: Cpu, Component: ModelDefaults },
    { required: {"methods":["escalate_effort","escalate_model","list_models","set_escalate_effort","set_escalate_model","set_model_favorite","set_title_effort","set_title_model","set_vision_effort","set_vision_model","title_effort","title_model","vision_effort","vision_model"],"topics":[]}, id: 'helpers', scope: 'machine', copyable: true, label: 'Helpers', icon: Wand2, Component: Helpers },
    { required: {"methods":["add_provider","list_logins","list_providers","local_servers_probe","login","logout","provider_models","refresh_provider_models","remove_provider","set_model_limits","set_secret"],"topics":["agent.login@1"]}, id: 'providers', scope: 'machine', copyable: true, copyNote: "API keys and logins are never copied.", label: 'Providers', icon: Cloud, Component: ModelProviders },
  ],
}
