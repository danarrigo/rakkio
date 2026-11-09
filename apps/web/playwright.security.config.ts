import { defineConfig } from '@playwright/test'
import config from './playwright.config'

export default defineConfig(config, {
  testIgnore: [],
  testMatch: [
    'security-headers.spec.ts',
    'indexing.spec.ts',
    'notes-files-settings-ui-audit.spec.ts',
    'calendar-ui-audit.spec.ts',
    'operations.spec.ts',
  ],
  use: { ...config.use, baseURL: 'http://127.0.0.1:4173' },
  webServer: {
    command: 'npm run build && npm run preview -- --host 127.0.0.1 --port 4173',
    url: 'http://127.0.0.1:4173',
    reuseExistingServer: false,
  },
})
