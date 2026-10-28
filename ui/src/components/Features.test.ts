// Features.test.ts - the newer UI pieces: kill-switch and update banners, RSS feed validation,
// search, and drag-to-reorder in the table.
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { get } from 'svelte/store';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import Banners from './Banners.svelte';
import RssEditor from './RssEditor.svelte';
import SearchDialog from './SearchDialog.svelte';
import TorrentTable from './TorrentTable.svelte';
import { addRequest, appStatus, filter, queueOrder, search, searchOpen, setAll, sort } from '../lib/store';
import type { AppStatus, Feed, Torrent } from '../lib/types';

const h = vi.hoisted(() => ({ api: {} as Record<string, ReturnType<typeof vi.fn>> }));

vi.mock('../lib/api', () => {
  const fn = vi.fn;
  h.api = {
    installUpdate: fn(async () => '1.1.0'),
    restartApp: fn(async () => {}),
    search: fn(async () => [
      { title: 'Ubuntu magnet', link: 'magnet:?xt=urn:btih:' + 'a'.repeat(40), size: 100, seeders: 9 },
      { title: 'Ubuntu file', link: 'https://example.org/u.torrent', size: null, seeders: null },
    ]),
    fetchTorrentUrl: fn(async () => 'C:\\tmp\\u.torrent'),
    moveInQueue: fn(async () => {}),
    torrentAction: fn(async () => {}),
  };
  return { api: h.api, inTauri: false };
});

/** A status with sensible defaults; `over` changes fields. */
function status(over: Partial<AppStatus> = {}): AppStatus {
  return { version: '1.0.0', proxy_down: false, killed: false, update: null, warnings: [], route: 'none', profile_name: null, ...over };
}

describe('banners', () => {
  beforeEach(() => appStatus.set(null));

  it('shows the kill-switch banner when everything is held', () => {
    appStatus.set(status({ proxy_down: true, killed: true }));
    render(Banners);
    expect(screen.getByRole('alert')).toHaveTextContent(/Proxy unreachable/);
    expect(screen.getByRole('alert')).toHaveTextContent(/paused/);
  });

  it('shows a softer banner when the proxy is down but the kill switch is off', () => {
    appStatus.set(status({ proxy_down: true }));
    render(Banners);
    expect(screen.getByRole('alert')).toHaveTextContent(/never connect directly/);
  });

  it('offers the update, installs it, then offers a restart', async () => {
    appStatus.set(status({ update: { version: '1.1.0', notes: 'n' } }));
    render(Banners);
    await fireEvent.click(screen.getByRole('button', { name: 'Update now' }));
    await waitFor(() => expect(h.api.installUpdate).toHaveBeenCalled());
    await fireEvent.click(await screen.findByRole('button', { name: 'Restart now' }));
    expect(h.api.restartApp).toHaveBeenCalled();
  });

  it('shows startup warnings and lets them be dismissed', async () => {
    appStatus.set(status({ warnings: ['Blocklist not loaded: x'] }));
    render(Banners);
    expect(screen.getByText('Blocklist not loaded: x')).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
    expect(screen.queryByText('Blocklist not loaded: x')).toBeNull();
  });
});

describe('rss editor', () => {
  it('refuses a non-http address and a broken filter, and only then enables Add', async () => {
    render(RssEditor, { feeds: [] as Feed[], categories: [], interval: 30 });
    const add = screen.getByRole('button', { name: 'Add' });
    await fireEvent.input(screen.getByLabelText('New feed name'), { target: { value: 'f' } });
    await fireEvent.input(screen.getByLabelText('New feed address'), { target: { value: 'ftp://x.example/feed' } });
    expect(add).toBeDisabled();
    expect(screen.getByRole('alert')).toHaveTextContent(/http/);
    await fireEvent.input(screen.getByLabelText('New feed address'), { target: { value: 'https://x.example/feed' } });
    await fireEvent.input(screen.getByLabelText('New feed filter'), { target: { value: '(' } });
    expect(add).toBeDisabled();
    expect(screen.getByRole('alert')).toHaveTextContent(/regular expression/);
    await fireEvent.input(screen.getByLabelText('New feed filter'), { target: { value: 'ubuntu' } });
    expect(add).toBeEnabled();
  });

  it('adds and removes feeds', async () => {
    const feeds: Feed[] = [];
    render(RssEditor, { feeds, categories: [], interval: 30 });
    await fireEvent.input(screen.getByLabelText('New feed name'), { target: { value: 'Mine' } });
    await fireEvent.input(screen.getByLabelText('New feed address'), { target: { value: 'https://x.example/feed' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Add' }));
    expect(await screen.findByLabelText('Feed address for Mine')).toBeInTheDocument();
    await fireEvent.click(screen.getByRole('button', { name: 'Remove Mine' }));
    expect(screen.queryByLabelText('Feed address for Mine')).toBeNull();
  });
});

describe('search', () => {
  beforeEach(() => {
    addRequest.set(null);
    searchOpen.set(true);
    Object.values(h.api).forEach((f) => f.mockClear());
  });

  it('lists results and hands a magnet to the add dialog', async () => {
    render(SearchDialog);
    await fireEvent.input(screen.getByLabelText('Search query'), { target: { value: 'ubuntu' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Search' }));
    expect(await screen.findByText('Ubuntu magnet')).toBeInTheDocument();
    await fireEvent.click(screen.getAllByRole('button', { name: 'Add' })[0]);
    expect(get(addRequest)).toEqual({ kind: 'magnet', uri: 'magnet:?xt=urn:btih:' + 'a'.repeat(40) });
    expect(get(searchOpen)).toBe(false);
  });

  it('downloads a .torrent link first, then opens the add dialog with the file', async () => {
    render(SearchDialog);
    await fireEvent.input(screen.getByLabelText('Search query'), { target: { value: 'ubuntu' } });
    await fireEvent.click(screen.getByRole('button', { name: 'Search' }));
    await screen.findByText('Ubuntu file');
    await fireEvent.click(screen.getAllByRole('button', { name: 'Add' })[1]);
    await waitFor(() => expect(h.api.fetchTorrentUrl).toHaveBeenCalledWith('https://example.org/u.torrent'));
    expect(get(addRequest)).toEqual({ kind: 'file', path: 'C:\\tmp\\u.torrent' });
  });

  it('Escape closes the dialog', async () => {
    render(SearchDialog);
    await fireEvent.keyDown(window, { key: 'Escape' });
    expect(get(searchOpen)).toBe(false);
  });
});

describe('queue reordering', () => {
  /** A torrent row. */
  function t(id: string): Torrent {
    return { id, name: `name ${id}`, status: 'downloading', error: null, progress: 0.1, down_rate: 0, up_rate: 0, size: 100, peers: 0, category: null };
  }

  beforeEach(() => {
    Object.values(h.api).forEach((f) => f.mockClear());
    filter.set('all');
    search.set('');
    sort.set({ key: 'queue', dir: 1 });
    setAll([t('a'), t('b'), t('c'), t('d')]);
    queueOrder.set(['a', 'b', 'c', 'd']);
    // jsdom has no layout or pointer capture: give the list a top of 0 and make capture a no-op.
    Element.prototype.getBoundingClientRect = () => ({ top: 0, bottom: 600, left: 0, right: 800, width: 800, height: 600, x: 0, y: 0, toJSON() {} }) as DOMRect;
    Element.prototype.setPointerCapture = () => {};
  });

  it('shows queue positions', () => {
    render(TorrentTable);
    const rows = screen.getAllByRole('row').filter((r) => r.classList.contains('item'));
    expect(rows.map((r) => r.querySelector('.qcell span')?.textContent)).toEqual(['1', '2', '3', '4']);
  });

  it('dragging a grip to another row moves the torrent in the queue', async () => {
    render(TorrentTable);
    const grip = screen.getByRole('button', { name: 'Drag to reorder name a' });
    await fireEvent.pointerDown(grip, { pointerId: 1, clientY: 10 });
    await fireEvent.pointerMove(grip, { pointerId: 1, clientY: 46 * 2 + 10 }); // over row index 2
    await fireEvent.pointerUp(grip, { pointerId: 1 });
    expect(h.api.moveInQueue).toHaveBeenCalledWith('a', 2);
  });

  it('does not reorder while the table is sorted by something else', async () => {
    sort.set({ key: 'name', dir: 1 });
    render(TorrentTable);
    const grip = screen.getByRole('button', { name: 'Drag to reorder name a' });
    await fireEvent.pointerDown(grip, { pointerId: 1, clientY: 10 });
    await fireEvent.pointerMove(grip, { pointerId: 1, clientY: 100 });
    await fireEvent.pointerUp(grip, { pointerId: 1 });
    expect(h.api.moveInQueue).not.toHaveBeenCalled();
  });
});
