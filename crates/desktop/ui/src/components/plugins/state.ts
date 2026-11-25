// 216 §5, contract §12.3: the words and dots of a plugin's lifecycle state. Frozen after B0.
import type { Dot, PluginRunState } from '@/rpc/bindings/method.plugins.output'

/** The state chip's text. */
export const STATE_LABEL: Record<PluginRunState, string> = {
  running: 'running',
  starting: 'starting',
  backoff: 'crashed',
  failed: 'failed',
  exited: 'exited',
  awaiting_approval: 'needs approval',
  disabled: 'disabled',
  invalid: 'invalid',
  stopping: 'stopping',
}

/** The state chip's dot. */
export const STATE_DOT: Record<PluginRunState, Dot> = {
  running: 'ok',
  starting: 'busy',
  stopping: 'busy',
  backoff: 'warn',
  failed: 'error',
  invalid: 'error',
  exited: 'off',
  disabled: 'off',
  awaiting_approval: 'off',
}
