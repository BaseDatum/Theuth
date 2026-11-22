/**
 * Identity stand-in for `dompurify` in the in-process mermaid bundle (no DOM). mermaid's parse path calls
 * `sanitize` on labels (`sanitizeText`) and registers two `addHook`s; with nothing rendered there is nothing to
 * sanitise, and the real DOMPurify without a `window` exposes no `sanitize` at all (`isSupported: false`).
 */
const DOMPurify = {
  isSupported: true,
  version: "theuth-identity",
  sanitize: (dirty: unknown): string => (typeof dirty === "string" ? dirty : String(dirty ?? "")),
  addHook(): void {},
  removeHook(): void {},
  removeHooks(): void {},
  removeAllHooks(): void {},
  setConfig(): void {},
  clearConfig(): void {},
  isValidAttribute: (): boolean => true,
};

export default DOMPurify;
