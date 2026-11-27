import { Bot, Eye, FlaskConical, Infinity as InfinityIcon, KeyRound, RefreshCw, ShieldCheck } from 'lucide-react'
import { SecretsSettingsTab } from '@/components/library/secrets/SettingsTab'
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
    { required: {"methods":["config_flag_get","config_flag_set","set_hint_limit","set_max_salvages","set_max_tool_rounds","set_peek_threshold","set_pty_idle","set_schedule_run_timeout","set_status_every","set_stream_idle","set_subagent_timeout"],"topics":[]}, id: 'loop', scope: 'machine', copyable: true, label: 'Loop', icon: RefreshCw, Component: Loop },
    { required: {"methods":["endless_settings_get","endless_settings_set","list_models","set_compact_style","set_model_favorite","test_summary_call","title_model"],"topics":[]}, id: 'memory', scope: 'machine', copyable: true, label: 'Context', icon: InfinityIcon, Component: Memory },
    { required: {"methods":["set_approvals"],"topics":[]}, id: 'approvals', scope: 'machine', copyable: true, label: 'Approvals', icon: ShieldCheck, Component: Approvals },
    // 221 §8: id `external-secrets` because `/settings/secrets` already redirects to the `/secrets` page (moved.ts).
    { required: {"methods":["get_external_settings","set_external_setting"],"topics":[]}, id: 'external-secrets', scope: 'machine', copyable: false, copyNote: "Secret stores are set up per machine.", label: 'Secrets', icon: KeyRound, Component: SecretsSettingsTab },
    { required: {"methods":["list_models","set_advisor_default_effort","set_advisor_default_model","set_advisor_immune_turns","set_advisor_max_consults","set_advisor_notes_per_update","set_advisor_sync_backlog","set_model_favorite"],"topics":[]}, id: 'advisor', scope: 'machine', copyable: true, label: 'Advisor', icon: Eye, Component: Advisor },
    { required: {"methods":["autoresearch.mechanism","autoresearch.settings.get","autoresearch.settings.set","list_models","list_schedules","set_model_favorite"],"topics":[]}, id: 'autoresearch', scope: 'machine', copyable: true, label: 'Autoresearch', icon: FlaskConical, Component: Autoresearch },
    { required: {"methods":["acp_login","add_acp","list_acp","list_acp_registry","remove_acp"],"topics":[]}, id: 'harnesses', scope: 'machine', copyable: false, copyNote: "Harness logins are per machine.", label: 'Harnesses', icon: Bot, Component: Agents },
  ],
}
