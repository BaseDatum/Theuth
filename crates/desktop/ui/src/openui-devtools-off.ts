/** OpenUI 1.x auto-mounts its Inspect widget in Vite development. theuth owns its own app tools, so opt out before
 * any OpenUI module evaluates; the flag is OpenUI's own auto-mount guard. */
;(globalThis as typeof globalThis & Record<symbol, boolean>)[Symbol.for('openui.devtools.autoMount')] = true
