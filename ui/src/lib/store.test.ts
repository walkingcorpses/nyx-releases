// store.test.ts - update handling, filtering and sorting.
import { get } from 'svelte/store';
import { beforeEach, describe, expect, it } from 'vitest';
import { applyUpdate, filter, search, setAll, sort, torrents, totals, visible } from './store';
import type { Torrent } from './types';

/** A torrent row with sensible defaults; `over` changes fields. */
function t(id: string, over: Partial<Torrent> = {}): Torrent {
  return {
    id, name: `name ${id}`, status: 'downloading', error: null, progress: 0.5,
    down_rate: 0, up_rate: 0, size: 100, peers: 0, category: null, ...over,
  };
}

describe('store', () => {
  beforeEach(() => {
    setAll([]);
    filter.set('all');
    search.set('');
    sort.set({ key: 'name', dir: 1 });
  });

  it('applyUpdate adds, replaces and removes by id', () => {
    applyUpdate({ changed: [t('a'), t('b')], removed: [] });
    expect([...get(torrents).keys()].sort()).toEqual(['a', 'b']);
    applyUpdate({ changed: [t('b', { progress: 0.9 })], removed: ['a'] });
    expect([...get(torrents).keys()]).toEqual(['b']);
    expect(get(torrents).get('b')!.progress).toBe(0.9);
  });

  it('filters by status chip and search text', () => {
    setAll([t('a', { status: 'seeding' }), t('b', { name: 'Ubuntu ISO' }), t('c', { status: 'error' })]);
    filter.set('seeding');
    expect(get(visible).map((x) => x.id)).toEqual(['a']);
    filter.set('all');
    search.set('ubuntu');
    expect(get(visible).map((x) => x.id)).toEqual(['b']);
  });

  it('sorts by column and direction with stable ties', () => {
    setAll([t('a', { size: 3 }), t('b', { size: 1 }), t('c', { size: 2 })]);
    sort.set({ key: 'size', dir: -1 });
    expect(get(visible).map((x) => x.id)).toEqual(['a', 'c', 'b']);
  });

  it('totals sums rates', () => {
    setAll([t('a', { down_rate: 10, up_rate: 1 }), t('b', { down_rate: 5, up_rate: 2 })]);
    expect(get(totals)).toEqual({ down: 15, up: 3 });
  });
});
