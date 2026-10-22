// types.ts - the shapes exchanged with the Rust backend (see src-tauri/src/dto.rs and
// nyx-core settings). Field names match the JSON the backend sends: snake_case.

/** Lifecycle word for a torrent row. */
export type Status = 'metadata' | 'queued' | 'checking' | 'downloading' | 'seeding' | 'paused' | 'error';

/** One row of the torrent table. `error` is set only when status is "error". */
export interface Torrent {
  id: string;
  name: string;
  status: Status;
  error: string | null;
  progress: number;
  down_rate: number;
  up_rate: number;
  size: number;
  peers: number;
  category: string | null;
}

/** A file inside a torrent; priority 0 means skip. */
export interface FileItem {
  path: string;
  size: number;
  progress: number;
  priority: number;
}

/** A connected peer. */
export interface Peer {
  addr: string;
  client: string;
  down_rate: number;
  up_rate: number;
  flags: string;
  encrypted: boolean;
}

/** A tracker with a short status. */
export interface Tracker {
  url: string;
  status: string;
}

/** What the detail drawer shows for one torrent. */
export interface Detail {
  files: FileItem[];
  peers: Peer[];
  trackers: Tracker[];
}

/** A .torrent inspected before adding. */
export interface Meta {
  id: string;
  name: string;
  files: FileItem[];
}

/** Payload of the `torrents://update` event. */
export interface UpdatePayload {
  changed: Torrent[];
  removed: string[];
}

/** SOCKS5 proxy settings; null in Settings means "no proxy". */
export interface Proxy {
  host: string;
  port: number;
  username: string | null;
  password: string | null;
}

/** One weekly schedule block: days is a bitmask (Mon = 1 ... Sun = 64); limits are bytes/s, 0 = unlimited. */
export interface Block {
  days: number;
  start: [number, number];
  end: [number, number];
  down: number;
  up: number;
}

/** A category: a name plus the folder its torrents save to. */
export interface Category {
  name: string;
  save_dir: string;
}

/** Everything the settings page edits. */
export interface Settings {
  encryption: 'Forced' | 'Prefer';
  proxy: Proxy | null;
  max_active_downloads: number;
  max_active_seeds: number;
  down_limit: number;
  up_limit: number;
  schedule: { blocks: Block[] };
  categories: Category[];
  default_save_dir: string;
  start_minimized: boolean;
  minimize_to_tray: boolean;
}

/** What the add dialog was asked to open: a magnet link, a .torrent file path, or an empty paste box. */
export type AddSource =
  | { kind: 'magnet'; uri: string }
  | { kind: 'file'; path: string }
  | { kind: 'paste' }; // the dialog first asks the user to paste a magnet link
