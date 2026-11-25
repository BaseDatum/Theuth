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
 * The latest published release from GitHub Releases, baked in at build time so the first paint links a real release;
 * at runtime the page refreshes from the GitHub Releases API. Offline, private-repo (404), or rate-limited builds
 * fall back to the product version in tauri.conf.json.
 */
const githubToken = process.env.GITHUB_TOKEN || process.env.GH_TOKEN
const released = await fetch('https://api.github.com/repos/BaseDatum/Theuth/releases/latest', {
  headers: {
    Accept: 'application/vnd.github+json',
    'User-Agent': 'theuth-site-build',
    ...(githubToken ? { Authorization: `Bearer ${githubToken}` } : {}),
  },
})
  .then((r) => (r.ok ? (r.json() as Promise<{ tag_name?: string }>) : null))
  .then((j) => j?.tag_name?.replace(/^v/, '') || version)
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
