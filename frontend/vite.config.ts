import path from 'node:path'
// `vitest/config` re-exports Vite's `defineConfig` widened with the `test`
// field, so one config file still drives both `vite build` and `vitest`.
import { defineConfig } from 'vitest/config'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

export default defineConfig({
  plugins: [react(), tailwindcss()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    proxy: {
      '/api': 'http://127.0.0.1:3000',
    },
  },
  test: {
    environment: 'jsdom',
    // Note: not `src/test/` — the repo's .gitignore has a bare `test` rule
    // that matches a directory of that name at any depth.
    setupFiles: ['./src/testing/setup.ts'],
    include: ['src/**/*.test.{ts,tsx}'],
  },
})
