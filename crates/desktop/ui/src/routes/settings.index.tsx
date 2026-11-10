import { Navigate, createFileRoute } from '@tanstack/react-router'
import { SettingsHome } from '@/components/settings/SettingsPage'
import { useIsMobile } from '@/hooks/use-mobile'

/** 188 S3: `/settings` is the category list on a phone; a wider window has the column beside every tab and opens Appearance. */
function Index() {
  return useIsMobile() ? <SettingsHome /> : <Navigate to="/settings/$tab" params={{ tab: 'appearance' }} replace />
}

export const Route = createFileRoute('/settings/')({ component: Index })
