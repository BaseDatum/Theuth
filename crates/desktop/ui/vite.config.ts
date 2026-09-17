import { readFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import tailwindcss from '@tailwindcss/vite'
import { tanstackRouter } from '@tanstack/router-plugin/vite'
import react from '@vitejs/plugin-react'
import { defineConfig } from 'vitest/config'

/** The product version: `just bump-version` keeps tauri.conf.json and Cargo.toml in lockstep. */
const { version } = JSON.parse(readFileSync(new URL('../tauri.conf.json', import.meta.url), 'utf8')) as { version: string }

export default defineConfig({
  define: { __APP_VERSION__: JSON.stringify(version) },
  plugins: [tanstackRouter({ target: 'react', autoCodeSplitting: true }), react(), tailwindcss()],
  resolve: { alias: { '@': fileURLToPath(new URL('./src', import.meta.url)) } },
  clearScreen: false,
  server: { port: 1420, strictPort: true },
  // Code-split routes pull deps the optimizer has not seen (codemirror, dialog
  // plugin); scanning every module up front avoids the mid-session full reload.
  optimizeDeps: { entries: ['index.html', 'src/**/*.{ts,tsx}'] },
  envPrefix: ['VITE_', 'TAURI_ENV_*'],
  build: { target: 'safari15' },
  test: { include: ['src/**/*.test.ts'] },
})
