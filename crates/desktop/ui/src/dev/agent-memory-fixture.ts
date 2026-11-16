import { type AgentInput, isIsolated } from '@/lib/agent-memory'
import { emptyDraft, toFile } from '@/components/agents/wizard'
import { makeApi } from '@/rpc/bindings/client'
import { receiveComponent } from '@/rpc/bindings/receive'
import type { AgentFile as WireFile } from '@/rpc/bindings/method.roster.input'

export const REFUSAL = 'isolated_memory: yang already has 147 notes in the main memory; hire a new agent with isolated memory instead (moving existing notes is not supported yet)'
export const MEMORY_EMPTY = 'Nothing remembered yet. What this agent records stays here and never reaches your main memory.'

/** Dev-only component fixtures: no engine, network, or main-memory fallback. */
export function memoryFixture(mode = 'notes') {
  const files: Record<string, AgentInput> = Object.fromEntries([
    ['theuth', 'theuth', false], ['research', 'Researcher', true], ['yang', 'SWE desk', false], ['sergei', 'SRE desk', false],
  ].map(([name, title, isolated]) => [String(name), toFile({ ...emptyDraft(), name: String(name), title: String(title), body: 'Research charter.', isolated_memory: isolated === true }, false)]))
  const notes = [
    ['decisions/analytics-vendor-shortlist', 'Vendor shortlist for the Q1 analytics tool'],
    ['facts/rag-planning-paper-notes', 'Paper: retrieval-augmented planning (2026)'],
    ['procedures/survey-export', 'How to export the survey results'],
  ].map(([id, title]) => receiveComponent('method.knowledge.output.Meta', { id: id!, title: title!, type: 'decision', description: '', tags: [], owner: 'agent:research', visibility: 'private', source: 'model', timestamp: '2026-11-14T12:00:00Z', scope: null, evidence: [] }))
  const calls: { method: string; params: unknown[] }[] = []
  const api = makeApi({
    async call<T>(method: string, params: unknown[]): Promise<T> {
      calls.push({ method, params })
      let result: unknown
      switch (method) {
        case 'agent_get': result = receiveComponent('method.roster.output.AgentFile', files[String(params[0])]!); break
        case 'agents_list': result = Object.values(files).map((file) => ({ ...receiveComponent('method.roster.output.AgentSummary', { file, status: 'idle', desk: null, open_tasks: 0, spent_month: 0 }), isolated_memory: isIsolated(file) })); break
        case 'agent_put': {
          const file = params[0] as WireFile
          if ((file.name === 'yang' || mode === 'refuse') && isIsolated(file)) throw new Error(REFUSAL)
          files[file.name] = file
          result = null
          break
        }
        case 'knowledge_list':
        case 'knowledge_search':
        case 'knowledge_get': {
          if (params.at(-1) !== 'agent:research' && params.at(-1) !== 'agent:yang') throw new Error('unknown bundle: fixture requires an agent bundle')
          if (mode === 'error') throw new Error('isolated memory unavailable here: fixture bundle is on another machine')
          if (method === 'knowledge_get') {
            const meta = notes.find((note) => note.id === params[0])
            if (!meta) throw new Error('no such concept')
            result = { meta, links: [[notes[1]!.id, true], ['facts/missing', false]], cited_by: [], text: `---\ntype: decision\n---\nThree vendors remain after the pricing pass: the two hosted options and the self-hosted one. The self-hosted option needs a database the team already runs.\n\nSee [planning paper](/facts/rag-planning-paper-notes) and [missing note](/facts/missing).` }
          } else result = mode === 'empty' ? [] : method === 'knowledge_search' ? notes.filter((note) => note.title.toLowerCase().includes(String(params[0]).toLowerCase())).map((meta) => ({ meta, snippet: '', score: 1 })) : notes
          break
        }
        case 'doc_ensure': result = 'fixture-charter'; break
        case 'doc_markdown': result = 'Research charter.'; break
        case 'engine_agents_list': case 'tasks_list': case 'list_sessions': case 'list_models': case 'list_acp': case 'list_mcp': case 'sandboxes_list': result = []; break
        case 'mesh_status': result = { self_id: 'fixture', self_name: 'this machine', peers: [] }; break
        default: throw new Error(`fixture: ${method} not answered`)
      }
      return result as T
    },
  })
  return { api, calls, files, notes }
}
