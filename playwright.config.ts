import { defineConfig } from "@playwright/test";

/**
 * Layout smoke tests for the browser-only M0 prototype.
 *
 * The prototype is served by Vite on its own port so it never collides with the
 * Tauri dev server (1420). Run `npx playwright install chromium` once before
 * `npm run test:e2e`.
 */
export default defineConfig({
  testDir: "./e2e",
  fullyParallel: true,
  forbidOnly: Boolean(process.env.CI),
  retries: process.env.CI ? 1 : 0,
  reporter: "list",
  use: {
    baseURL: "http://127.0.0.1:4177",
    trace: "retain-on-failure",
  },
  webServer: {
    command:
      "npm run dev -- --host 127.0.0.1 --port 4177 --strictPort",
    url: "http://127.0.0.1:4177/prototype.html",
    reuseExistingServer: !process.env.CI,
    timeout: 60_000,
  },
  projects: [
    {
      name: "desktop-1440x900",
      use: { viewport: { width: 1440, height: 900 } },
    },
    {
      name: "desktop-1280x720",
      use: { viewport: { width: 1280, height: 720 } },
    },
  ],
});
