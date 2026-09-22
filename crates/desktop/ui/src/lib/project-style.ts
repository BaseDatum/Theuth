import { useQuery } from '@tanstack/react-query'
import {
  Book,
  Briefcase,
  Bug,
  Camera,
  Cloud,
  Code,
  Database,
  FileText,
  Flag,
  FlaskConical,
  Folder,
  Globe,
  Heart,
  House,
  type LucideIcon,
  MessageSquare,
  Mic,
  Music,
  Rocket,
  Server,
  Shield,
  Star,
  Terminal,
  Wrench,
  Zap,
} from 'lucide-react'
import { queryKeys, useApi } from '@/lib/rpc'
import type { ProjectStyle } from '@/rpc/bindings/ProjectStyle'

/**
 * Project icon and colour (51 §3 as-built). The server stores keys from these two lists and nothing else;
 * `crates/agent/src/project.rs` (`ICONS`, `COLORS`) is a copy of the key sets here — change both together.
 */
export const ICONS: Record<string, LucideIcon> = {
  Folder,
  Briefcase,
  Code,
  Terminal,
  Bug,
  Database,
  Server,
  Cloud,
  Globe,
  Book,
  FileText,
  MessageSquare,
  Mic,
  Music,
  Camera,
  Heart,
  Star,
  Flag,
  Zap,
  Rocket,
  Wrench,
  FlaskConical,
  Shield,
  House,
}

/** Palette key → Tailwind text colour, one step lighter in the dark theme so it reads on soot. */
export const COLORS: Record<string, string> = {
  red: 'text-red-600 dark:text-red-400',
  orange: 'text-orange-600 dark:text-orange-400',
  amber: 'text-amber-600 dark:text-amber-400',
  green: 'text-green-600 dark:text-green-400',
  teal: 'text-teal-600 dark:text-teal-400',
  blue: 'text-blue-600 dark:text-blue-400',
  indigo: 'text-indigo-600 dark:text-indigo-400',
  violet: 'text-violet-600 dark:text-violet-400',
  pink: 'text-pink-600 dark:text-pink-400',
  gray: 'text-gray-500 dark:text-gray-400',
}

/** The icon for `name`; unknown or unset falls back to the neutral `Folder`, so every row keeps its glyph slot. */
export const iconFor = (name: string | null | undefined): LucideIcon => (name && ICONS[name]) || Folder

/** The colour class for `key`; unknown or unset is the tertiary foreground. */
export const colorFor = (key: string | null | undefined): string => (key && COLORS[key]) || 'text-fg-tertiary'

const byName = (rows: ProjectStyle[]): Record<string, ProjectStyle> =>
  Object.fromEntries(rows.map((r) => [r.name, r]))

/** Every styled project by name (`list_project_styles`); setters invalidate `queryKeys.projectStyles`. */
export function useProjectStyles(): Record<string, ProjectStyle> {
  const api = useApi()
  const { data } = useQuery({
    queryKey: queryKeys.projectStyles,
    queryFn: () => api.list_project_styles(),
    select: byName,
  })
  return data ?? {}
}
