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
import type { ProjectStyle } from "@/rpc/bindings/method.agent.output"

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

/** Palette key → `text-project-<key>` (25 §2.4 as-built); the `--project-*` variable switches with the theme. */
export const COLORS: Record<string, string> = Object.fromEntries(
  ['ochre', 'carnelian', 'malachite', 'faience', 'lapis', 'nile', 'umber', 'slate', 'sand', 'plum'].map((k) => [k, `text-project-${k}`]),
)

/** Pre-palette Tailwind keys still stored on old rows → their mineral replacement. The server accepts both. */
export const LEGACY_COLORS: Record<string, string> = {
  red: 'carnelian',
  orange: 'ochre',
  amber: 'sand',
  green: 'malachite',
  teal: 'faience',
  blue: 'lapis',
  indigo: 'slate',
  violet: 'plum',
  pink: 'plum',
  gray: 'umber',
}

/** The icon for `name`; unknown or unset falls back to the neutral `Folder`, so every row keeps its glyph slot. */
export const iconFor = (name: string | null | undefined): LucideIcon => (name && ICONS[name]) || Folder

/** The colour class for `key` (legacy keys map first); unknown or unset is the tertiary foreground. */
export const colorFor = (key: string | null | undefined): string =>
  (key && COLORS[LEGACY_COLORS[key] ?? key]) || 'text-fg-tertiary'

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
