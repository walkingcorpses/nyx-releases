// restart.spec.ts - the "restart after an update" path of the real exe: restart_app starts a new
// process that waits for the old one (NYX_WAIT_PID) so single-instance does not turn it away, and
// the new instance comes up with the same data folder. The new process is ended through the
// tray-quit test hook, so nothing is killed by name.
import { chromium, expect, test } from '@playwright/test';
import { existsSync, writeFileSync } from 'node:fs';
import path from 'node:path';
import { attach, launch } from './helpers';

test('restart_app hands over to a new instance that starts after the old one exits', async () => {
  const inst = await launch();
  const first = await attach(inst.port);
  const page = first.contexts()[0].pages()[0] ?? (await first.contexts()[0].waitForEvent('page'));
  await expect(page.getByText('Nothing here yet')).toBeVisible();

  const exited = new Promise<void>((res) => inst.child.on('exit', () => res()));
  // Same call the "Restart now" button makes.
  await page.evaluate(() => (window as any).__TAURI_INTERNALS__.invoke('restart_app'));
  await Promise.race([exited, new Promise((_, rej) => setTimeout(() => rej(new Error('old instance did not exit')), 20_000))]);
  await first.close().catch(() => {});

  // The replacement inherits the debugging port; once it is up, the UI is there again.
  const second = await attach(inst.port);
  const page2 = second.contexts()[0].pages()[0] ?? (await second.contexts()[0].waitForEvent('page'));
  await expect(page2.getByText('Nothing here yet')).toBeVisible({ timeout: 30_000 });
  expect(existsSync(path.join(inst.root, 'data', 'engine'))).toBe(true);

  // Stop the replacement politely (it is not our child, so no kill by PID is possible).
  writeFileSync(path.join(inst.root, 'data', 'test-tray-quit'), '');
  await expect.poll(async () => {
    try { await fetch(`http://127.0.0.1:${inst.port}/json/version`); return 'up'; } catch { return 'down'; }
  }, { timeout: 20_000 }).toBe('down');
  await second.close().catch(() => {});
  void chromium;
});
