import { readFileSync } from 'node:fs'
import tailwindcss from '@tailwindcss/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vite'

/** The product version: `just bump-version` keeps tauri.conf.json and Cargo.toml in lockstep. */
const { version } = JSON.parse(readFileSync(new URL('../crates/desktop/tauri.conf.json', import.meta.url), 'utf8')) as {
  version: string
}

export default defineConfig({
  define: { __APP_VERSION__: JSON.stringify(version) },
  plugins: [react(), tailwindcss()],
  clearScreen: false,
  server: { port: 1430, strictPort: true },
  build: { target: 'safari15' },
})
