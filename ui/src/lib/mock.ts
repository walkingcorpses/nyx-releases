// mock.ts - an in-memory stand-in for the Rust backend, used when the UI runs in a plain browser
// (npm run dev) so it can be developed and checked visually. Torrents download over time, so
// the progress bars and speed graph move. `?rows=1000` in the URL loads 1,000 rows for a
// scroll-performance check.

import type { AppUpdate, Detail, FileItem, Meta, NetworkStatus, Profile, SearchResult, Settings, AppStatus, Status, Torrent, UpdatePayload } from './types';

const NAMES = [
  'ubuntu-26.04-desktop-amd64.iso', 'Big Buck Bunny (2008) 1080p', 'Sintel.2010.4k.HDR',
  'blender-5.1-windows-x64.zip', 'archlinux-2026.10.01-x86_64.iso', 'Public Domain Music Pack – Vol. 3',
  'Debian 13 netinst', 'Open Game Art – Sprites', 'Wikipedia dump 2026-09', 'Linux kernel sources 7.1',
];

/** Deterministic pseudo-random so the preview looks the same every reload. */
function rng(seed: number): () => number {
  let x = seed;
  return () => {
    x = (x * 1664525 + 1013904223) % 4294967296;
    return x / 4294967296;
  };
}

const rows = Number(new URLSearchParams(globalThis.location?.search ?? '').get('rows')) || 14;
const rand = rng(7);
const STATUSES: Status[] = ['downloading', 'downloading', 'downloading', 'seeding', 'paused', 'queued', 'checking', 'error'];

let list: Torrent[] = Array.from({ length: rows }, (_, i) => {
  const status = STATUSES[i % STATUSES.length];
  const size = Math.floor((0.2 + rand() * 6) * 1024 ** 3);
  return {
    id: i.toString(16).padStart(40, '0'),
    name: `${NAMES[i % NAMES.length]}${i >= NAMES.length ? ` #${i}` : ''}`,
    status,
    error: status === 'error' ? 'Cannot use save folder: access denied' : null,
    progress: status === 'seeding' ? 1 : rand() * 0.95,
    down_rate: 0, up_rate: 0, size, peers: 0,
    category: i % 4 === 0 ? 'movies' : i % 5 === 0 ? 'software' : null,
  };
});

let settings: Settings = {
  encryption: 'Forced', proxy: null, max_active_downloads: 3, max_active_seeds: 3,
  down_limit: 0, up_limit: 0, schedule: { blocks: [] },
  categories: [{ name: 'movies', save_dir: 'D:\\Media\\Movies' }, { name: 'software', save_dir: 'D:\\Software' }],
  default_save_dir: 'D:\\Downloads', start_minimized: false, minimize_to_tray: true,
  kill_switch: true, bind_interface: null, blocklist: null, check_updates: false,
  rss: { feeds: [{ name: 'Linux ISOs', url: 'https://example.org/feed.xml', filter: 'ubuntu|debian', category: 'software', enabled: true }], interval_minutes: 30 },
  search_url: '',
  network: { active_profile: 'p1', rotation: { enabled: false, pool: [], min_minutes: 5, max_minutes: 10 } },
};
// Two fake VPN profiles for the browser preview; the real backend never sends keys.
let profiles: Profile[] = [
  { id: 'p1', name: 'Mullvad Amsterdam', group: 'Mullvad', endpoint: '193.32.126.66:51820', active: true },
  { id: 'p2', name: 'Mullvad Zurich', group: 'Mullvad', endpoint: '185.65.134.80:51820', active: false },
];
let order: string[] = list.map((t) => t.id);

const subs = new Set<(u: UpdatePayload) => void>();

/** Moves progress and rates along once a second and pushes the changed rows. */
function tick(): void {
  const changed: Torrent[] = [];
  list = list.map((t) => {
    if (t.status === 'downloading') {
      const down = Math.floor(200_000 + rand() * 4_000_000);
      const progress = Math.min(1, t.progress + down / t.size);
      const next = { ...t, down_rate: down, up_rate: Math.floor(rand() * 300_000), peers: 4 + Math.floor(rand() * 40), progress };
      if (progress >= 1) { next.status = 'seeding'; next.down_rate = 0; }
      changed.push(next);
      return next;
    }
    if (t.status === 'seeding') {
      const next = { ...t, up_rate: Math.floor(rand() * 800_000), peers: 2 + Math.floor(rand() * 12) };
      changed.push(next);
      return next;
    }
    return t;
  });
  subs.forEach((cb) => cb({ changed, removed: [], queue: null }));
}
if (typeof window !== 'undefined') setInterval(tick, 1000);

/** A fake file list for any torrent. */
function files(seed: number): FileItem[] {
  return [
    { path: 'Release/video.mkv', size: 4_200_000_000, progress: 0.4, priority: 4 },
    { path: 'Release/subs/en.srt', size: 90_000, progress: 1, priority: 4 },
    { path: 'Release/subs/fr.srt', size: 88_000, progress: 1, priority: 0 },
    { path: 'Release/extras/making-of.mp4', size: 900_000_000 + seed, progress: 0.1, priority: 4 },
    { path: 'Release/README.txt', size: 1200, progress: 1, priority: 4 },
  ];
}

/** Mock implementation of the whole Api surface. */
export const mockApi = {
  listTorrents: async () => list,
  addMagnet: async (uri: string) => {
    const id = (list.length + 1).toString(16).padStart(40, 'a');
    list = [{ id, name: 'Fetching metadata…', status: 'metadata', error: null, progress: 0, down_rate: 0, up_rate: 0, size: 0, peers: 0, category: null }, ...list];
    void uri;
    return id;
  },
  addTorrent: async () => (list.length + 1).toString(16).padStart(40, 'b'),
  inspectTorrent: async (path: string): Promise<Meta> => ({ id: 'f'.repeat(40), name: path.split(/[\\/]/).pop() ?? 'torrent', files: files(0) }),
  torrentAction: async (id: string, action: string) => {
    if (action === 'remove' || action === 'remove_delete') list = list.filter((t) => t.id !== id);
    if (action === 'pause') list = list.map((t) => (t.id === id ? { ...t, status: 'paused', down_rate: 0, up_rate: 0 } : t));
    if (action === 'resume') list = list.map((t) => (t.id === id ? { ...t, status: t.progress >= 1 ? 'seeding' : 'downloading' } : t));
    subs.forEach((cb) => cb({ changed: list.filter((t) => t.id === id), removed: list.some((t) => t.id === id) ? [] : [id], queue: null }));
  },
  setPriorities: async () => {},
  setTorrentOptions: async () => {},
  setTorrentLimits: async () => {},
  getDetail: async (id: string): Promise<Detail> => ({
    files: files(id.length),
    peers: Array.from({ length: 6 }, (_, i) => ({
      addr: `203.0.113.${10 + i}:${51400 + i}`, client: ['qBittorrent 5.0', 'Transmission 4.0', 'Deluge 2.1', 'libtorrent 2.0'][i % 4],
      down_rate: Math.floor(rand() * 900_000), up_rate: Math.floor(rand() * 100_000),
      flags: i % 2 ? 'IcE' : 'iCE', encrypted: i !== 3,
    })),
    trackers: [
      { url: 'udp://tracker.opentrackr.org:1337/announce', status: 'Working' },
      { url: 'udp://open.stealth.si:80/announce', status: 'Error: timed out' },
    ],
    down_limit: 0, up_limit: 250 * 1024,
  }),
  getMagnet: async (id: string) => `magnet:?xt=urn:btih:${id}`,
  moveInQueue: async (id: string, position: number) => {
    order = order.filter((x) => x !== id);
    order.splice(Math.min(position, order.length), 0, id);
    subs.forEach((cb) => cb({ changed: [], removed: [], queue: [...order] }));
  },
  getQueueOrder: async () => order,
  getStatus: async (): Promise<AppStatus> => ({
    version: '1.0.0',
    proxy_down: new URLSearchParams(globalThis.location?.search ?? '').has('proxydown'),
    killed: new URLSearchParams(globalThis.location?.search ?? '').has('proxydown'),
    update: new URLSearchParams(globalThis.location?.search ?? '').has('update') ? { version: '1.1.0', notes: 'Faster everything.' } : null,
    warnings: [],
    route: 'up',
    profile_name: 'Mullvad Amsterdam',
  }),
  checkUpdate: async (): Promise<AppUpdate | null> => null,
  installUpdate: async () => '1.1.0',
  restartApp: async () => {},
  rssRunNow: async () => ({ added: 0, baselined: 1, errors: [] }),
  search: async (query: string): Promise<SearchResult[]> => [
    { title: `${query} 26.04 Desktop amd64`, link: 'magnet:?xt=urn:btih:' + 'a'.repeat(40), size: 6_200_000_000, seeders: 412 },
    { title: `${query} Server 26.04`, link: 'https://example.org/x.torrent', size: 2_100_000_000, seeders: 87 },
  ],
  fetchTorrentUrl: async () => 'D:\\Downloads\\fetched.torrent',
  listProfiles: async () => profiles.map((p) => ({ ...p, active: p.id === settings.network.active_profile })),
  importProfile: async (_path: string, name: string, group: string): Promise<Profile> => {
    const p = { id: 'p' + (profiles.length + 1), name, group, endpoint: '203.0.113.9:51820', active: false };
    profiles = [...profiles, p];
    return p;
  },
  renameProfile: async (id: string, name: string, group: string) => {
    profiles = profiles.map((p) => (p.id === id ? { ...p, name, group } : p));
  },
  deleteProfile: async (id: string) => { profiles = profiles.filter((p) => p.id !== id); },
  testProfile: async (): Promise<NetworkStatus> => ({
    route: 'up', profile: 'Mullvad Amsterdam', handshake_age_secs: 1, rx_bytes: 4096, tx_bytes: 2048,
    exit_ip: '193.32.126.66', next_rotation_secs: null,
  }),
  getNetworkStatus: async (): Promise<NetworkStatus> => ({
    route: 'up', profile: 'Mullvad Amsterdam', handshake_age_secs: 12, rx_bytes: 52_428_800, tx_bytes: 3_145_728,
    exit_ip: null, next_rotation_secs: settings.network.rotation.enabled ? 421 : null,
  }),
  rotateNow: async () => 'p2',
  getSettings: async () => settings,
  setSettings: async (s: Settings) => { settings = s; },
  registerAssociations: async () => {},
  unregisterAssociations: async () => {},
  takePendingArgs: async () => [] as string[],
  onUpdate: async (cb: (u: UpdatePayload) => void) => { subs.add(cb); return () => { subs.delete(cb); }; },
  onOpen: async () => () => {},
  onDrop: async () => () => {},
  onDragState: async () => () => {},
  pickFile: async () => 'D:\\lists\\blocklist.txt',
  pickTorrentFile: async () => 'D:\\Downloads\\example.torrent',
  pickFolder: async () => 'D:\\Downloads',
  windowMinimize: async () => {},
  windowToggleMaximize: async () => {},
  windowClose: async () => {},
};
