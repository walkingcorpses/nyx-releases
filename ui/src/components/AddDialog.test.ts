// AddDialog.test.ts - the add flow end to end with a mocked backend: pasting a magnet, dropping a
// .torrent, choosing files, the sequential toggle and the save-folder guard.
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import AddHost from './AddHost.svelte';
import { addRequest } from '../lib/store';

// Hoisted so the mock factory can see them. dropHandlers collects what AddHost registers.
const h = vi.hoisted(() => ({
  dropHandlers: [] as Array<(paths: string[]) => void>,
  api: {} as Record<string, ReturnType<typeof vi.fn>>,
}));

vi.mock('../lib/api', () => {
  const fn = vi.fn;
  h.api = {
    getSettings: fn(async () => ({
      default_save_dir: 'D:\\Downloads',
      categories: [{ name: 'movies', save_dir: 'D:\\Media\\Movies' }],
    })),
    inspectTorrent: fn(async () => ({
      id: 'a'.repeat(40),
      name: 'Pack',
      files: [
        { path: 'Pack/a.txt', size: 10, progress: 0, priority: 4 },
        { path: 'Pack/b.txt', size: 20, progress: 0, priority: 4 },
      ],
    })),
    addMagnet: fn(async () => 'm'.repeat(40)),
    addTorrent: fn(async () => 't'.repeat(40)),
    setTorrentOptions: fn(async () => {}),
    setPriorities: fn(async () => {}),
    torrentAction: fn(async () => {}),
    getDetail: fn(async () => ({ files: [], peers: [], trackers: [] })),
    pickFolder: fn(async () => null),
    takePendingArgs: fn(async () => []),
    onOpen: fn(async () => () => {}),
    onDragState: fn(async () => () => {}),
    onDrop: fn(async (cb: (paths: string[]) => void) => {
      h.dropHandlers.push(cb);
      return () => {};
    }),
  };
  return { api: h.api, inTauri: false };
});

const MAGNET = 'magnet:?xt=urn:btih:' + 'ab'.repeat(20) + '&dn=My+Film';

/** Fires a window-level paste event carrying `text`, as Ctrl+V would. */
function paste(text: string): void {
  const ev = new Event('paste', { bubbles: true, cancelable: true });
  Object.assign(ev, { clipboardData: { getData: () => text } });
  window.dispatchEvent(ev);
}

describe('add flow', () => {
  beforeEach(() => {
    addRequest.set(null);
    h.dropHandlers.length = 0;
    Object.values(h.api).forEach((f) => f.mockClear());
  });

  it('pasting a magnet opens dialog with the magnet name', async () => {
    render(AddHost);
    paste(MAGNET);
    expect(await screen.findByText('My Film')).toBeInTheDocument();
    expect(screen.getByText('Fetching metadata…')).toBeInTheDocument();
    await waitFor(() => expect(h.api.addMagnet).toHaveBeenCalledWith(MAGNET, '', null, false));
  });

  it('dropping a .torrent opens dialog with its file tree', async () => {
    render(AddHost);
    await waitFor(() => expect(h.dropHandlers.length).toBe(1));
    h.dropHandlers[0](['D:\\x\\pack.torrent']);
    expect(await screen.findByLabelText('a.txt')).toBeInTheDocument();
    expect(screen.getByLabelText('b.txt')).toBeInTheDocument();
    expect(h.api.inspectTorrent).toHaveBeenCalledWith('D:\\x\\pack.torrent');
  });

  it('unchecking a file sends priority 0 for it', async () => {
    render(AddHost);
    addRequest.set({ kind: 'file', path: 'D:\\x\\pack.torrent' });
    await fireEvent.click(await screen.findByLabelText('b.txt'));
    await fireEvent.click(screen.getByRole('button', { name: 'Add' }));
    await waitFor(() => expect(h.api.setPriorities).toHaveBeenCalledWith('t'.repeat(40), [4, 0]));
    expect(h.api.torrentAction).toHaveBeenCalledWith('t'.repeat(40), 'resume');
  });

  it('sequential toggle is passed through', async () => {
    render(AddHost);
    addRequest.set({ kind: 'file', path: 'D:\\x\\pack.torrent' });
    await screen.findByLabelText('a.txt');
    await fireEvent.click(screen.getByLabelText('Sequential download'));
    await fireEvent.click(screen.getByRole('button', { name: 'Add' }));
    await waitFor(() => expect(h.api.torrentAction).toHaveBeenCalledWith('t'.repeat(40), 'sequential_on'));
  });

  it('Add is disabled while save folder is empty', async () => {
    render(AddHost);
    addRequest.set({ kind: 'file', path: 'D:\\x\\pack.torrent' });
    await screen.findByLabelText('a.txt');
    const add = screen.getByRole('button', { name: 'Add' });
    expect(add).toBeEnabled();
    await fireEvent.input(screen.getByLabelText('Save folder'), { target: { value: '   ' } });
    expect(add).toBeDisabled();
  });
});
