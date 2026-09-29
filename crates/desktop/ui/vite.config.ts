import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import tailwindcss from '@tailwindcss/vite'
import { tanstackRouter } from '@tanstack/router-plugin/vite'
import react from '@vitejs/plugin-react'
import typegpu from 'unplugin-typegpu/vite'
import { type Plugin, normalizePath, searchForWorkspaceRoot } from 'vite'
import { defineConfig } from 'vitest/config'

/** The product version: `just bump-version` keeps tauri.conf.json and Cargo.toml in lockstep. */
const { version } = JSON.parse(readFileSync(new URL('../tauri.conf.json', import.meta.url), 'utf8')) as { version: string }

/** `@theuth/apps` (46) is consumed from its repo sources, not installed: no copy to go stale, and its `node_modules`
 * holds the one pinned, patched OpenUI install every `@openuidev/*` import resolves to (tsconfig `paths` mirrors this). */
const apps = fileURLToPath(new URL('../../../packages/apps', import.meta.url))
/** `@theuth/documents` (59) likewise: the block schema and YAML helper are the helper's own sources. */
const documents = fileURLToPath(new URL('../../../packages/documents', import.meta.url))

/**
 * `importer` lies under `dir`. Vite hands importers with forward slashes and, on Windows, a drive letter whose case
 * differs between sources (`C:/…` from the file system, `c:/…` from resolved ids), so a bare `startsWith` on the
 * `fileURLToPath` form silently never matched there — `packages/documents` then pulled its own headless
 * `@blocknote/core` + `prosemirror-view`, and the Windows bundle carried two `DecorationSet` classes
 * ("Cannot read properties of undefined (reading 'localsInner')" on every note open).
 */
const under = (importer: string | undefined, dir: string): boolean => {
  if (!importer) return false
  const a = normalizePath(importer)
  const b = `${normalizePath(dir)}/`
  return process.platform === 'win32' ? a.toLowerCase().startsWith(b.toLowerCase()) : a.startsWith(b)
}

/**
 * Resolve every `@openuidev/*` import as if it came from inside `packages/apps`, through Vite's own resolver: the
 * package `exports` maps (import condition, subpaths such as `./genui-lib` and `./layered/styles/index.css`) apply,
 * and only the patched ESM builds under `packages/apps/node_modules` are ever loaded, whoever imports them.
 */
const openui = (): Plugin => ({
  name: 'theuth:openui-single-instance',
  enforce: 'pre',
  resolveId(source, importer, options) {
    if (!source.startsWith('@openuidev/')) return null
    if (under(importer, apps)) return null
    return this.resolve(source, `${apps}/src/index.ts`, { ...options, skipSelf: true })
  },
})

/**
 * The opposite rule for `@theuth/documents`: its sources import `@blocknote/core`, which must be the UI's copy (the
 * package's own `node_modules` holds a second, headless install), so bare imports from under `packages/documents`
 * resolve as if written in `src/`.
 */
const documentsDeps = (): Plugin => ({
  name: 'theuth:documents-shared-deps',
  enforce: 'pre',
  resolveId(source, importer, options) {
    if (!under(importer, documents)) return null
    if (source.startsWith('.') || source.startsWith('/')) return null
    return this.resolve(source, fileURLToPath(new URL('./src/main.tsx', import.meta.url)), { ...options, skipSelf: true })
  },
})

export default defineConfig({
  define: { __APP_VERSION__: JSON.stringify(version) },
  plugins: [
    openui(),
    documentsDeps(),
    tanstackRouter({ target: 'react', autoCodeSplitting: true }),
    react(),
    tailwindcss(),
    // The talk orb's shaders (76 §0.12, `@shadercn/orb-20`): every `'use gpu'` body needs the TypeGPU transform or
    // it fails at resolve time with "Missing metadata for tgpu.fn function body". Shader code lives in `.ts` only.
    typegpu({ include: [/\/components\/orbs\/.*\.ts$/, /\/lib\/shader\.ts$/] }),
  ],
  resolve: {
    alias: [
      { find: '@', replacement: fileURLToPath(new URL('./src', import.meta.url)) },
      { find: /^@theuth\/apps$/, replacement: `${apps}/src/index.ts` },
      { find: /^@theuth\/apps\/renderer$/, replacement: `${apps}/src/renderer.tsx` },
      { find: /^@theuth\/documents\/schema$/, replacement: `${documents}/src/schema.ts` },
      { find: /^@theuth\/documents\/yaml$/, replacement: `${documents}/src/yaml.ts` },
    ],
    // The packages' `node_modules` carry their own React/zod/zustand and (documents) a headless BlockNote with the
    // whole ProseMirror/Yjs family; one copy each, ours — two `prosemirror-view`s break every editor.
    dedupe: [
      'react', 'react-dom', 'zod', 'zustand',
      'yjs', 'y-prosemirror', 'y-protocols', '@tiptap/pm', '@tiptap/core',
      'prosemirror-model', 'prosemirror-state', 'prosemirror-view', 'prosemirror-transform', 'prosemirror-commands',
      'prosemirror-history', 'prosemirror-keymap', 'prosemirror-inputrules', 'prosemirror-dropcursor', 'prosemirror-gapcursor',
      'prosemirror-schema-list', 'prosemirror-tables', 'prosemirror-changeset', 'prosemirror-highlight',
    ],
  },
  clearScreen: false,
  server: { port: 1420, strictPort: true, fs: { allow: [searchForWorkspaceRoot(process.cwd()), apps, documents] } },
  // Code-split routes pull deps the optimizer has not seen (codemirror, dialog
  // plugin); scanning every module up front avoids the mid-session full reload.
  optimizeDeps: { entries: ['index.html', 'src/**/*.{ts,tsx}'] },
  envPrefix: ['VITE_', 'TAURI_ENV_*'],
  build: { target: 'safari17' },
  // The apps package's OpenUI deps go through Vite (not node's resolver) so `dedupe` gives them our React in tests too.
  test: { include: ['src/**/*.test.ts'], server: { deps: { inline: [/packages\/apps\/node_modules/] } } },
})
