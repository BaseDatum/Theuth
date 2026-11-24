import { notify as toast } from '@/lib/notify'

/** OpenUI's typed `@CopyToClipboard(text)` action, handled locally by every app host. */
export function copyToClipboard(params: Record<string, unknown>): void {
  const text = params.text
  if (typeof text !== 'string') {
    toast.error('Nothing to copy')
    return
  }
  void navigator.clipboard.writeText(text).then(
    () => toast.success('Copied'),
    (e: unknown) => toast.error(e instanceof Error ? e.message : String(e)),
  )
}
