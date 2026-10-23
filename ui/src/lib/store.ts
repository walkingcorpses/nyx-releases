// store.ts - the UI's shared state: the torrent map (kept current by backend update events),
// selection, filter, sort and settings. No torrent logic lives here, only bookkeeping.

import { derived, get, writable } from 'svelte/store';
import type { AddSource, AppStatus, Settings, Status, Torrent, UpdatePayload } from './types';

/** All torrents by id. Replaced (never mutated) on each update so subscribers re-render. */
export const torrents = writable<Map<string, Torrent>>(new Map());

/** Replaces the whole map (initial load). */
export function setAll(list: Torrent[]): void {
  torrents.set(new Map(list.map((t) => [t.id, t])));
}

/** Queue order (ids, first to start first). Kept current by updates. */
export const queueOrder = writable<string[]>([]);

/** Network guard / update / warning state, polled by App. */
export const appStatus = writable<AppStatus | null>(null);

/** True while a file is being dragged over the window (shows the drop hint). */
export const dragging = writable(false);

/** Set to open the search dialog. */
export const searchOpen = writable(false);

/** Applies one backend update: adds or replaces `changed` rows and drops `removed` ids. */
export function applyUpdate(u: UpdatePayload): void {
  if (u.queue) queueOrder.set(u.queue);
  torrents.update((m) => {
    const next = new Map(m);
    for (const t of u.changed) next.set(t.id, t);
    for (const id of u.removed) next.delete(id);
    return next;
  });
}

/** Id of the selected torrent (drives the detail drawer), or null. */
export const selectedId = writable<string | null>(null);

/** Status filter chips. */
export type Filter = 'all' | 'downloading' | 'seeding' | 'paused' | 'error';
export const filter = writable<Filter>('all');

/** Free-text search over names. */
export const search = writable('');

/** Sort column and direction. */
export type SortKey = 'queue' | 'name' | 'size' | 'progress' | 'down_rate' | 'up_rate' | 'peers' | 'status';
export const sort = writable<{ key: SortKey; dir: 1 | -1 }>({ key: 'queue', dir: 1 });

/** Current settings (loaded from the backend), null until then. */
export const settings = writable<Settings | null>(null);

/** Set to open the add dialog for a magnet or file; null closes it. */
export const addRequest = writable<AddSource | null>(null);

/** Whether the settings page is open. */
export const settingsOpen = writable(false);

/** Does a torrent pass the status filter chip? */
export function matchesFilter(t: Torrent, f: Filter): boolean {
  switch (f) {
    case 'all':
      return true;
    case 'downloading':
      return ['downloading', 'metadata', 'checking', 'queued'].includes(t.status);
    case 'seeding':
      return t.status === 'seeding';
    case 'paused':
      return t.status === 'paused';
    case 'error':
      return t.status === 'error';
  }
}

const STATUS_ORDER: Record<Status, number> = {
  downloading: 0, metadata: 1, checking: 2, queued: 3, seeding: 4, paused: 5, error: 6,
};

/** Sorts a copy of the list by the chosen column. */
export function sortTorrents(list: Torrent[], key: SortKey, dir: 1 | -1, order: string[] = []): Torrent[] {
  const place = new Map(order.map((id, i) => [id, i]));
  const cmp = (a: Torrent, b: Torrent): number => {
    if (key === 'queue') return (place.get(a.id) ?? 1e9) - (place.get(b.id) ?? 1e9);
    if (key === 'name') return a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' });
    if (key === 'status') return STATUS_ORDER[a.status] - STATUS_ORDER[b.status];
    return a[key] - b[key];
  };
  return [...list].sort((a, b) => dir * cmp(a, b) || a.id.localeCompare(b.id));
}

/** The rows the table shows: filtered, searched and sorted. */
export const visible = derived([torrents, filter, search, sort, queueOrder], ([$t, $f, $q, $s, $o]) => {
  const q = $q.trim().toLowerCase();
  const rows = [...$t.values()].filter((t) => matchesFilter(t, $f) && (!q || t.name.toLowerCase().includes(q)));
  return sortTorrents(rows, $s.key, $s.dir, $o);
});

/** Total download and upload rate across all torrents (status bar). */
export const totals = derived(torrents, ($t) => {
  let down = 0;
  let up = 0;
  for (const t of $t.values()) {
    down += t.down_rate;
    up += t.up_rate;
  }
  return { down, up };
});

/** The selected torrent row, if it still exists. */
export const selected = derived([torrents, selectedId], ([$t, $id]) => ($id ? ($t.get($id) ?? null) : null));

/** Convenience for non-component code: current map as an array. */
export function allTorrents(): Torrent[] {
  return [...get(torrents).values()];
}
