// smoke.spec.ts - end-to-end smoke test of the real nyx.exe.
// It starts the exe off-screen and muted with a throwaway data folder, attaches Playwright to its
// WebView2 over the remote-debugging port, and checks: (a) title bar + empty state, (b) adding a
// .torrent through the add dialog shows a row, (c) settings show Encryption = Forced, (d) the tray
// "Quit" action ends the process. Only the process this test spawned is ever killed (by PID).
import { expect, test, type Browser, type Page } from '@playwright/test';
import { spawn } from 'node:child_process';
import { writeFileSync } from 'node:fs';
import path from 'node:path';
import { attach, exePath, launch, makeTorrent, type Instance } from './helpers';

let inst: Instance;
let browser: Browser;
let page: Page;
let root: string;
let env: NodeJS.ProcessEnv;
let child: Instance['child'];

test.beforeAll(async () => {
  inst = await launch();
  ({ root, env, child } = inst);
  browser = await attach(inst.port);
  const ctx = browser.contexts()[0];
  page = ctx.pages()[0] ?? (await ctx.waitForEvent('page'));
});

test.afterAll(async () => {
  await browser?.close().catch(() => {});
  // Kill only the process we started, and only if the quit test did not already end it.
  if (child && child.exitCode === null) child.kill();
});

test('nyx smoke: empty state, add torrent, settings, tray quit', async () => {
  // Any console error or uncaught page error (including a Tauri permission denial) fails the test.
  const problems: string[] = [];
  page.on('pageerror', (e) => problems.push(`pageerror: ${e.message}`));
  page.on('console', (m) => m.type() === 'error' && problems.push(`console: ${m.text()}`));

  // (a) title bar and empty state
  await expect(page.locator('header').getByText('Nyx', { exact: true })).toBeVisible();
  await expect(page.getByText('Nothing here yet')).toBeVisible();

  // (b) a second launch with a .torrent argument is forwarded to this instance and opens the
  // add dialog; confirming it adds a row.
  const torrent = makeTorrent(root);
  const dl = path.join(root, 'dl');
  const second = spawn(exePath(), [torrent], { env, stdio: 'ignore' });
  await new Promise<void>((res) => second.on('exit', () => res()));
  const dialog = page.getByRole('dialog', { name: 'Add torrent' });
  await expect(dialog).toBeVisible({ timeout: 15_000 });
  await expect(dialog.getByLabel('e2e-sample.bin')).toBeVisible();
  await dialog.getByLabel('Save folder').fill(dl);
  await dialog.getByRole('button', { name: 'Add', exact: true }).click();
  await expect(dialog).toBeHidden();
  await expect(page.locator('.item').filter({ hasText: 'e2e-sample.bin' })).toHaveCount(1, { timeout: 10_000 });

  // The detail drawer loads files from the backend, and the per-torrent actions are accepted.
  await page.locator('.item').filter({ hasText: 'e2e-sample.bin' }).click();
  await expect(page.getByRole('region', { name: 'Torrent details' }).getByText('e2e-sample.bin').first()).toBeVisible();
  await page.getByRole('button', { name: /More/ }).click();
  await page.getByRole('button', { name: 'Move to front of queue' }).click();
  await page.getByRole('button', { name: /More/ }).click();
  await page.getByRole('button', { name: /Speed limit/ }).click();
  await page.getByLabel('Torrent download limit').fill('500');
  await page.getByRole('button', { name: 'Apply' }).click();
  await expect(page.getByRole('dialog', { name: 'Torrent speed limits' })).toBeHidden();

  // The queue column numbers the torrent, and search without a provider explains what to do.
  await expect(page.locator('.item').first().locator('.qcell span')).toHaveText('1');
  await page.getByRole('button', { name: 'Search…' }).click();
  await page.getByLabel('Search query').fill('ubuntu');
  await page.getByRole('button', { name: 'Search', exact: true }).click();
  await expect(page.getByRole('alert')).toContainText('no search provider');
  await page.keyboard.press('Escape');
  await expect(page.getByRole('dialog', { name: 'Search torrents' })).toBeHidden();

  // (c) settings: encryption defaults to Forced; saving round-trips through the backend
  await page.getByRole('button', { name: 'Settings' }).click();
  await expect(page.getByRole('radio', { name: /Forced/ })).toBeChecked();
  await page.getByRole('tab', { name: 'Queue' }).click();
  await page.getByLabel('Max active downloads').fill('5');
  await page.getByRole('button', { name: 'Save' }).click();
  await expect(page.getByRole('dialog', { name: 'Settings' })).toBeHidden();
  await page.getByRole('button', { name: 'Settings' }).click();
  await page.getByRole('tab', { name: 'Queue' }).click();
  await expect(page.getByLabel('Max active downloads')).toHaveValue('5');
  await page.getByRole('button', { name: 'Cancel' }).click();
  expect(problems).toEqual([]);

  // (d) Quit from the tray menu ends the process (the menu handler is triggered through the
  // test hook file, so no real menu pops up on the desktop)
  const exited = new Promise<number | null>((res) => child.on('exit', (code) => res(code)));
  writeFileSync(path.join(root, 'data', 'test-tray-quit'), '');
  const code = await Promise.race([exited, new Promise<'timeout'>((r) => setTimeout(() => r('timeout'), 15_000))]);
  expect(code).not.toBe('timeout');
});
