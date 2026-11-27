import { Brain, Moon, Sparkles, UserRoundCheck } from 'lucide-react'
import { Double } from '@/components/settings/Double'
import { Dreaming } from '@/components/settings/Dreaming'
import { Labs } from '@/components/settings/Labs'
import type { SettingsCategory } from '@/components/settings/types'

// 188: Learning (docs/design/188-settings-full-page.md); Decision models is today's Labs, always listed (F4).
export const learning: SettingsCategory = {
  id: 'learning',
  label: 'Learning',
  icon: Sparkles,
  tabs: [
    { required: {"methods":["dreaming.nights","dreaming.proposal_act","dreaming.proposals","dreaming.run_now","dreaming.set_settings","dreaming.settings","dreaming.status","dreaming.undo","list_models","list_schedules","set_model_favorite","subagent_model","title_model"],"topics":["dreaming.finished@1","dreaming.started@1","dreaming.undone@1"]}, id: 'dreaming', scope: 'hub', copyable: false, label: 'Dreaming', icon: Moon, Component: Dreaming },
    { required: {"methods":["config_flag_get","config_flag_set","double_class","double_classes","double_delete_all","double_export","double_forget","double_pause","double_rollback","double_set_cap","double_settings_get","double_settings_set","double_status","double_train","mesh_status","put_attachment","put_session_attachment"],"topics":["double.training@1","double.version@1"]}, id: 'double', scope: 'hub', copyable: false, label: 'Double', icon: UserRoundCheck, Component: Double },
    { required: {"methods":["config_flag_get","config_flag_set","decision_loaded","decision_mode","decision_stats","decision_training","delete_decision_training","set_decision_mode","set_decision_share","set_decision_training"],"topics":[]}, id: 'decision', scope: 'machine', copyable: false, copyNote: "Trained on this machine's data.", label: 'Decision models', icon: Brain, Component: Labs },
  ],
}
