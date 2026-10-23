// api.ts - the only file that talks to the Rust backend: one typed function per Tauri command,
// plus the update/open subscriptions and the native pickers. Outside Tauri (a plain browser
// tab during development) it falls back to the in-memory mock in mock.ts, so the UI can be
// built and looked at without the backend.

import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import { getCurrentWindow } from '@tauri-apps/api/window';
import { getCurrentWebview } from '@tauri-apps/api/webview';
import { open as openDialog } from '@tauri-apps/plugin-dialog';
import { mockApi } from './mock';
import type { AppUpdate, Detail, Meta, RssReport, SearchResult, Settings, AppStatus, Torrent, UpdatePayload } from './types';

/** True when running inside the Tauri webview. */
export const inTauri = typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;

/** Real backend: every function maps to one registered Tauri command. */
const real = {
  listTorrents: () => invoke<Torrent[]>('list_torrents'),
  addMagnet: (uri: string, savePath: string, category: string | null, paused: boolean) =>
    invoke<string>('add_magnet', { uri, savePath, category, paused }),
  addTorrent: (path: string, savePath: string, category: string | null, paused: boolean) =>
    invoke<string>('add_torrent', { path, savePath, category, paused }),
  inspectTorrent: (path: string) => invoke<Meta>('inspect_torrent', { path }),
  torrentAction: (id: string, action: string) => invoke<void>('torrent_action', { id, action }),
  setPriorities: (id: string, priorities: number[]) => invoke<void>('set_priorities', { id, priorities }),
  setTorrentOptions: (id: string, savePath: string, category: string | null) =>
    invoke<void>('set_torrent_options', { id, savePath, category }),
  setTorrentLimits: (id: string, down: number, up: number) => invoke<void>('set_torrent_limits', { id, down, up }),
  getDetail: (id: string) => invoke<Detail>('get_detail', { id }),
  getMagnet: (id: string) => invoke<string>('get_magnet', { id }),
  moveInQueue: (id: string, position: number) => invoke<void>('move_in_queue', { id, position }),
  getSettings: () => invoke<Settings>('get_settings'),
  setSettings: (settings: Settings) => invoke<void>('set_settings', { settings }),
  registerAssociations: () => invoke<void>('register_associations'),
  unregisterAssociations: () => invoke<void>('unregister_associations'),
  takePendingArgs: () => invoke<string[]>('take_pending_args'),
  getStatus: () => invoke<AppStatus>('get_status'),
  getQueueOrder: () => invoke<string[]>('get_queue_order'),
  checkUpdate: () => invoke<AppUpdate | null>('check_update'),
  installUpdate: () => invoke<string>('install_update'),
  restartApp: () => invoke<void>('restart_app'),
  rssRunNow: () => invoke<RssReport>('rss_run_now'),
  search: (query: string) => invoke<SearchResult[]>('search', { query }),
  fetchTorrentUrl: (url: string) => invoke<string>('fetch_torrent_url', { url }),

  /** Subscribes to backend update events; returns an unsubscribe function. */
  async onUpdate(cb: (u: UpdatePayload) => void): Promise<() => void> {
    return listen<UpdatePayload>('torrents://update', (e) => cb(e.payload));
  },
  /** Subscribes to "something was opened with Nyx" (magnet click, second launch). */
  async onOpen(cb: () => void): Promise<() => void> {
    return listen('app://open', () => cb());
  },
  /** Subscribes to files dropped on the window; the callback gets their paths. */
  async onDrop(cb: (paths: string[]) => void): Promise<() => void> {
    return getCurrentWebview().onDragDropEvent((e) => {
      if (e.payload.type === 'drop') cb(e.payload.paths);
    });
  },
  /** Subscribes to "a file is being dragged over the window" (true) and "it left or dropped" (false). */
  async onDragState(cb: (over: boolean) => void): Promise<() => void> {
    return getCurrentWebview().onDragDropEvent((e) => {
      const t = e.payload.type;
      if (t === 'enter' || t === 'over') cb(true);
      else cb(false);
    });
  },
  /** Native file picker for any single file (blocklists); null when cancelled. */
  async pickFile(): Promise<string | null> {
    const r = await openDialog({ multiple: false });
    return typeof r === 'string' ? r : null;
  },
  /** Native file picker for .torrent files; null when cancelled. */
  async pickTorrentFile(): Promise<string | null> {
    const r = await openDialog({ multiple: false, filters: [{ name: 'Torrent', extensions: ['torrent'] }] });
    return typeof r === 'string' ? r : null;
  },
  /** Native folder picker; null when cancelled. */
  async pickFolder(): Promise<string | null> {
    const r = await openDialog({ directory: true, multiple: false });
    return typeof r === 'string' ? r : null;
  },
  /** Custom title bar buttons. */
  windowMinimize: () => getCurrentWindow().minimize(),
  windowToggleMaximize: () => getCurrentWindow().toggleMaximize(),
  windowClose: () => getCurrentWindow().close(),
};

/** The API the components use. Same shape either way. */
export type Api = typeof real;
export const api: Api = inTauri ? real : (mockApi as Api);
