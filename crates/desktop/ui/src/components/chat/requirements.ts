import type { Requirements } from '@/rpc/discovery'

export const broadcastRequirements = { methods: ['broadcast', 'list_sessions'] } as const satisfies Requirements
export const terminalRequirements = {"methods":["cell_attach","cell_detach","cell_input","list_terminals","mesh_status","pty_input","pty_resize","subscribe_pty","unsubscribe_pty"],"topics":[]} as const satisfies Requirements
export const privateRequirements = { fields: [{ method: 'create_session', parameter: 'options', path: ['retention'], value: 'private' }, { method: 'create_session', parameter: 'options', path: ['view'] }] } as const satisfies Requirements
export const endlessRequirements = { methods: ['endless_settings_get', 'session_memory'], topics: ['agent.endless-progress@1'], fields: [{ method: 'create_session', parameter: 'options', path: ['memory'], value: 'endless' }] } as const satisfies Requirements

export const memoryRequirements = {"methods":["endless_fold","endless_pause","endless_recall","endless_retry","endless_settings_get","endless_zoom","list_models","session_memory","set_model_favorite","set_session_compactor_effort","set_session_compactor_model","set_session_view_bytes","test_summary_call"],"topics":["agent.endless-progress@1"]} as const satisfies Requirements
export const sessionCostRequirements = {"methods":["session_cost"],"topics":[]} as const satisfies Requirements
export const messagingRequirements = {"methods":["answer_peer_request","get_communication","list_peer_messages","list_peers","peer_message","set_peer_scope"],"topics":["agent.communication@1","agent.peer-message@1","agent.peer-delivered@1"]} as const satisfies Requirements
export const toolsRequirements = {"methods":["add_mcp_repo","add_mcp_server","list_mcp","mcp_login","mcp_logout","mcp_status","mesh_status","remove_mcp_server","set_mcp_gate","set_mcp_hidden","set_mcp_home","sync_mcp"],"topics":["tools.mcp-progress@1"]} as const satisfies Requirements
export const contextRequirements = {"methods":["list_models"],"topics":[]} as const satisfies Requirements
export const browserRequirements = {"methods":["browser_screenshot"],"topics":["browser.events@1"]} as const satisfies Requirements
export const approvalsRequirements = {"methods":["double_forget","double_take_over","list_sessions","mesh_status","resolve_approval"],"topics":[]} as const satisfies Requirements
export const modelRequirements = {"methods":["advisor_status","harness_options","list_models","list_sessions","resume_child","set_advisor","set_effort","set_harness_option","set_model","set_model_favorite"],"topics":["agent.events@1"]} as const satisfies Requirements
export const sessionMenuRequirements = {"methods":["cancel_job","get_approvals","get_communication","get_prompt","list_jobs","list_sessions","load_session","rename_session","session_move","set_approvals","set_communication"],"topics":[]} as const satisfies Requirements
