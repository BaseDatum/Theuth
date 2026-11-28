// Design 120 §4.2 / §14: the two Tauri commands of `crates/desktop/src/pages.rs` (`rename_all = "snake_case"`). The
// shell hands back the platform URL (`theuth-page://localhost/<token>` or `http://theuth-page.localhost/<token>`); the
// UI never builds one. Outside Tauri (`isTauri()` false) `HtmlPage` uses `srcdoc` instead and never calls these.

import { invoke } from '@tauri-apps/api/core'

export type PageProfile = 'standard' | 'deck'

/** Register the bootstrapped page under `call_id`; resolves to the URL to frame. */
export const pagePut = (call_id: string, html: string, profile?: PageProfile): Promise<string> =>
  invoke<string>('page_put', profile ? { call_id, html, profile } : { call_id, html })

/** Evict the page; a later load of its URL is a 404. */
export const pageDrop = (call_id: string): Promise<void> => invoke('page_drop', { call_id })
