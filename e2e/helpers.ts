// helpers.ts - shared by the e2e specs: finds the exe, starts it isolated and off-screen, and
// attaches Playwright to its WebView2 over the remote-debugging port.
import { chromium, type Browser } from '@playwright/test';
import { spawn, type ChildProcess } from 'node:child_process';
import { createHash } from 'node:crypto';
import { existsSync, mkdirSync, mkdtempSync, writeFileSync } from 'node:fs';
import net from 'node:net';
import { tmpdir } from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';

export const here = path.dirname(fileURLToPath(import.meta.url));

/** The exe under test: NYX_EXE, else dist/nyx.exe (build script output), else the cargo release build. */
export function exePath(): string {
  const candidates = [process.env.NYX_EXE, path.join(here, '..', 'dist', 'nyx.exe'), path.join(here, '..', 'target', 'release', 'nyx.exe')];
  const found = candidates.find((p) => p && existsSync(p));
  if (!found) throw new Error('nyx.exe not found. Run scripts/build-release.ps1 first, or set NYX_EXE.');
  return found;
}

/** Asks the OS for a free TCP port (for the debugging endpoint). */
export function freePort(): Promise<number> {
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
export function makeTorrent(dir: string): string {
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
export async function attach(port: number): Promise<Browser> {
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


/** A running test instance: its process, data folder, debug port and the environment it was started with. */
export interface Instance {
  child: ChildProcess;
  root: string;
  port: number;
  env: NodeJS.ProcessEnv;
}

/** Starts nyx.exe off-screen and muted with its own data folder and WebView2 profile. */
export async function launch(extraArgs: string[] = []): Promise<Instance> {
  const root = mkdtempSync(path.join(tmpdir(), 'nyx-e2e-'));
  const data = path.join(root, 'data');
  mkdirSync(data, { recursive: true });
  const port = await freePort();
  const env = {
    ...process.env,
    NYX_DATA_DIR: data,
    NYX_LISTEN: '127.0.0.1:0',
    NYX_NO_DISCOVERY: '1',
    NYX_TEST_OFFSCREEN: '1',
    NYX_TEST_LOG: path.join(root, 'log.txt'),
    WEBVIEW2_USER_DATA_FOLDER: path.join(root, 'webview'),
    WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS: `--remote-debugging-port=${port} --mute-audio`,
  };
  const child = spawn(exePath(), extraArgs, { env, stdio: 'ignore' });
  return { child, root, port, env };
}
