import { getCurrentWindow } from '@tauri-apps/api/window'
import { isPermissionGranted, requestPermission, sendNotification } from '@tauri-apps/plugin-notification'
import { isTauri } from '@/lib/tauri'

/** 25 §4.5: a system notification only when the user is not looking at the window. */
export async function notifyIfHidden(title: string, body: string): Promise<void> {
  if (!isTauri()) return
  if (document.hasFocus() && (await getCurrentWindow().isVisible())) return
  const granted = (await isPermissionGranted()) || (await requestPermission()) === 'granted'
  if (granted) sendNotification({ title, body })
}
