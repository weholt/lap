import path from 'path'

import vue from '@vitejs/plugin-vue'
import { defineConfig } from 'vitest/config'

// Test runner for Lap's Vue frontend. Mirrors the '@' alias from
// vite.config.js and keeps app build plugins out of the test pipeline.
export default defineConfig({
  plugins: [vue()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, './src'),
    },
  },
  test: {
    environment: 'happy-dom',
    include: ['tests/**/*.test.{js,ts}'],
  },
})
