// smoke.spec.ts - end-to-end smoke test of the real nyx.exe.
// It starts the exe off-screen and muted with a throwaway data folder, attaches Playwright to its
// WebView2 over the remote-debugging port, and checks: (a) title bar + empty state, (b) adding a
// .torrent through the add dialog shows a row, (c) settings show Encryption = Forced, (d) the tray
// "Quit" action ends the process. Only the process this test spawned is ever killed (by PID).
import { chromium, expect, test, type Browser, type Page } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import net from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

const here = path.dirname(fileURLToPath(import.meta.url));

/** The exe under test: NYX_EXE, else dist/nyx.exe (build script output), else the cargo release build. */
function exePath(): string {
  const candidates = [process.env.NYX_EXE, path.join(here, '..', 'dist', 'nyx.exe'), path.join(here, '..', 'target', 'release', 'nyx.exe')];
  const found = candidates.find((p) => p && existsSync(p));
  if (!found) throw new Error('nyx.exe not found. Run scripts/build-release.ps1 first, or set NYX_EXE.');
  return found;
}

/** Asks the OS for a free TCP port (for the debugging endpoint). */
function freePort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const s = net.createServer();
    s.listen(0, '127.0.0.1', () => {
      const p = (s.address() as net.AddressInfo).port;
      s.close(() => resolve(p));
    });
    s.on('error', reject);
  });
}

/** bencode for the little subset a single-file .torrent needs. */
function bencode(v: unknown): Buffer {
  if (Buffer.isBuffer(v)) return Buffer.concat([Buffer.from(`${v.length}:`), v]);
  if (typeof v === 'string') return bencode(Buffer.from(v));
  if (typeof v === 'number') return Buffer.from(`i${v}e`);
  const o = v as Record<string, unknown>;
  const parts = Object.keys(o).sort().flatMap((k) => [bencode(k), bencode(o[k])]);
  return Buffer.concat([Buffer.from('d'), ...parts, Buffer.from('e')]);
}

/** Writes a valid 3-piece single-file .torrent named `e2e-sample.bin` and returns its path. */
function makeTorrent(dir: string): string {
  const data = Buffer.alloc(40_000, 7);
  const pieceLen = 16_384;
  const pieces = Buffer.concat(
    [0, 1, 2].map((i) => createHash('sha1').update(data.subarray(i * pieceLen, (i + 1) * pieceLen)).digest()),
  );
  const info = { name: 'e2e-sample.bin', length: data.length, 'piece length': pieceLen, pieces };
  const file = path.join(dir, 'sample.torrent');
  writeFileSync(file, bencode({ info }));
  return file;
}

/** Waits until the debugging endpoint answers, then returns a Playwright browser attached to it. */
async function attach(port: number): Promise<Browser> {
  const deadline = Date.now() + 40_000;
  for (;;) {
    try {
      const r = await fetch(`http://127.0.0.1:${port}/json/version`);
      if (r.ok) break;
    } catch {
      /* not up yet */
    }
    if (Date.now() > deadline) throw new Error('WebView2 debugging port never came up');
    await new Promise((r) => setTimeout(r, 300));
  }
  return chromium.connectOverCDP(`http://127.0.0.1:${port}`);
}

let child: ChildProcess;
let browser: Browser;
let page: Page;
let root: string;
let env: NodeJS.ProcessEnv;

test.beforeAll(async () => {
  root = mkdtempSync(path.join(tmpdir(), 'nyx-e2e-'));
  const data = path.join(root, 'data');
  mkdirSync(data, { recursive: true });
  const port = await freePort();
  env = {
    ...process.env,
    NYX_DATA_DIR: data,
    NYX_LISTEN: '127.0.0.1:0',
    NYX_NO_DISCOVERY: '1',
    NYX_TEST_OFFSCREEN: '1',
    NYX_TEST_LOG: path.join(root, 'log.txt'),
    WEBVIEW2_USER_DATA_FOLDER: path.join(root, 'webview'),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --mute-audio`,
  };
  child = spawn(exePath(), [], { env, stdio: 'ignore' });
  browser = await attach(port);
  const ctx = browser.contexts()[0];
  page = ctx.pages()[0] ?? (await ctx.waitForEvent('page'));
});

test.afterAll(async () => {
  await browser?.close().catch(() => {});
  // Kill only the process we started, and only if the quit test did not already end it.
  if (child && child.exitCode === null) child.kill();
});

test('nyx smoke: empty state, add torrent, settings, tray quit', async () => {
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

  // (c) settings: encryption defaults to Forced
  await page.getByRole('button', { name: 'Settings' }).click();
  await expect(page.getByRole('radio', { name: /Forced/ })).toBeChecked();
  await page.getByRole('button', { name: 'Cancel' }).click();

  // (d) Quit from the tray menu ends the process (the menu handler is triggered through the
  // test hook file, so no real menu pops up on the desktop)
  const exited = new Promise<number | null>((res) => child.on('exit', (code) => res(code)));
  writeFileSync(path.join(root, 'data', 'test-tray-quit'), '');
  const code = await Promise.race([exited, new Promise<'timeout'>((r) => setTimeout(() => r('timeout'), 15_000))]);
  expect(code).not.toBe('timeout');
});
