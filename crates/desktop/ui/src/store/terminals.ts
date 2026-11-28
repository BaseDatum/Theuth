import { create } from 'zustand'

/** A user-created standalone node terminal (an engine shell, never a chat and never a job). */
export type UserTerminal = {
  /** Mesh id (`mesh_status.self_id` or a peer id). */
  engine: string
  /** Fresh shell id (never `"main"`), so two terminals on the same node never collide. */
  shell: string
  /** Display label the owner sees: the node name plus "terminal". */
  name: string
  /** Node display name. */
  node: string
}

/** Stable `<engine>:<shell>` identity for keys, deduping, and attach markers. */
export const terminalKey = (t: Pick<UserTerminal, 'engine' | 'shell'>): string => `${t.engine}:${t.shell}`

/** Default display label: `<node> terminal`. */
export const terminalName = (node: string): string => `${node.trim() || 'This machine'} terminal`

/** Existing route `/terminal/$engine/$shell` with `?name=` for opening a workspace pane. */
export const terminalPath = (t: Pick<UserTerminal, 'engine' | 'shell' | 'name'>): string =>
  `/terminal/${encodeURIComponent(t.engine)}/${encodeURIComponent(t.shell)}${t.name ? `?name=${encodeURIComponent(t.name)}` : ''}`

let shellSeq = 0

/** Generate a fresh shell id (never `"main"`), so two terminals on one node never collide. */
export function freshShellId(): string {
  const suffix =
    typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function'
      ? crypto.randomUUID().slice(0, 8)
      : `${Date.now().toString(36)}-${(++shellSeq).toString(36)}`
  return `term-${suffix}`
}

type UserTerminalsStore = {
  terminals: UserTerminal[]
  add(terminal: UserTerminal): void
  remove(engine: string, shell: string): void
  clear(): void
}

/** Client store of user terminals created this session, shared by the sidebar and composer attach picker. */
export const useUserTerminals = create<UserTerminalsStore>()((set) => ({
  terminals: [],
  add: (terminal) =>
    set((state) => ({
      terminals: [
        terminal,
        ...state.terminals.filter((t) => !(t.engine === terminal.engine && t.shell === terminal.shell)),
      ],
    })),
  remove: (engine, shell) =>
    set((state) => ({
      terminals: state.terminals.filter((t) => !(t.engine === engine && t.shell === shell)),
    })),
  clear: () => set({ terminals: [] }),
}))
