// playwright.config.ts - settings for the end-to-end smoke test. It drives the real nyx.exe over
// the WebView2 debugging port, so there is no browser to launch and no project matrix.
import { defineConfig } from '@playwright/test';

export default defineConfig({
  testDir: '.',
  testMatch: /.*\.spec\.ts/,
  timeout: 120_000,
  workers: 1,
  reporter: 'list',
});
