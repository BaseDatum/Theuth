import { readFileSync } from 'node:fs'
import { reactRouter } from '@react-router/dev/vite'
import tailwindcss from '@tailwindcss/vite'
import { fumadocsMdx } from 'fumadocs-mdx/vite'
import { defineConfig } from 'vite'

/** The product version: `just bump-version` keeps tauri.conf.json and Cargo.toml in lockstep. */
const { version } = JSON.parse(readFileSync(new URL('../crates/desktop/tauri.conf.json', import.meta.url), 'utf8')) as {
  version: string
}

/**
 * The released version from the bucket's latest.json, baked in at build time so the first paint (and dev/preview on
 * localhost, which the bucket's CORS policy does not allow) links a real release; on theuth.io the runtime fetch of
 * latest.json still refreshes it. Offline builds fall back to the product version.
 */
const released = await fetch('https://s3.rwx.dev/theuth/releases/latest.json')
  .then((r) => r.json() as Promise<{ version: string }>)
  .then((j) => j.version)
  .catch(() => version)

export default defineConfig({
  define: { __APP_VERSION__: JSON.stringify(version), __RELEASE_VERSION__: JSON.stringify(released) },
  // fumadocsMdx compiles content/docs (the `defineDocs` macro in src/lib/source.ts); reactRouter owns the
  // SPA + prerender build (react-router.config.ts) and React Fast Refresh.
  plugins: [fumadocsMdx(), tailwindcss(), reactRouter()],
  clearScreen: false,
  server: { port: 1430, strictPort: true },
  build: { target: 'safari15' },
})
