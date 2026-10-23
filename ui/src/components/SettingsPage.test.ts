// SettingsPage.test.ts - the settings editor and the pieces inside it: encryption default, proxy
// validation, the MSE limitation notice, the schedule editor and the peer encryption flag in the drawer.
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { describe, expect, it, vi } from 'vitest';
import DetailDrawer from './DetailDrawer.svelte';
import ScheduleEditor from './ScheduleEditor.svelte';
import SettingsPage from './SettingsPage.svelte';
import type { Settings, Torrent } from '../lib/types';

vi.mock('../lib/api', () => ({
  api: {
    getDetail: vi.fn(async () => ({
      files: [],
      peers: [
        { addr: '203.0.113.1:6881', client: 'qBittorrent', down_rate: 1, up_rate: 0, flags: 'IcE', encrypted: true },
        { addr: '203.0.113.2:6881', client: 'Old', down_rate: 0, up_rate: 0, flags: 'IcR', encrypted: false },
      ],
      trackers: [],
    })),
    pickFolder: vi.fn(async () => null),
    registerAssociations: vi.fn(async () => {}),
    unregisterAssociations: vi.fn(async () => {}),
  },
  inTauri: false,
}));

/** Default settings as the backend would send them. */
function base(): Settings {
  return {
    encryption: 'Forced', proxy: null, max_active_downloads: 3, max_active_seeds: 3,
    down_limit: 0, up_limit: 0, schedule: { blocks: [] }, categories: [],
    default_save_dir: 'D:\\Downloads', start_minimized: false, minimize_to_tray: true,
    kill_switch: true, bind_interface: null, blocklist: null, check_updates: false,
    rss: { feeds: [], interval_minutes: 30 }, search_url: '',
  };
}

describe('settings page', () => {
  it('encryption selector defaults to Forced', () => {
    render(SettingsPage, { settings: base(), onsave: async () => {}, onclose: () => {} });
    expect(screen.getByRole('radio', { name: /Forced/ })).toBeChecked();
    expect(screen.getByRole('radio', { name: /Prefer encrypted/ })).not.toBeChecked();
  });

  it('proxy form requires host and port when enabled', async () => {
    const onsave = vi.fn(async () => {});
    render(SettingsPage, { settings: base(), onsave, onclose: () => {} });
    const save = screen.getByRole('button', { name: 'Save' });
    expect(save).toBeEnabled();
    await fireEvent.click(screen.getByLabelText(/Use a SOCKS5 proxy/));
    expect(save).toBeDisabled();
    expect(screen.getByRole('alert')).toHaveTextContent(/host and a port/);
    await fireEvent.input(screen.getByLabelText('Proxy host'), { target: { value: '127.0.0.1' } });
    expect(save).toBeEnabled();
    await fireEvent.click(save);
    await waitFor(() =>
      expect(onsave).toHaveBeenCalledWith(expect.objectContaining({ proxy: expect.objectContaining({ host: '127.0.0.1', port: 1080 }) })),
    );
  });

  it('settings page shows the MSE limitation text', () => {
    render(SettingsPage, { settings: base(), onsave: async () => {}, onclose: () => {} });
    expect(screen.getByText(/does not hide your IP address from other peers in a swarm/)).toBeInTheDocument();
    expect(screen.getByText(/obfuscation, not authentication/)).toBeInTheDocument();
  });

  it('schedule editor rejects end equal to start', async () => {
    const blocks = [{ days: 1, start: [9, 0] as [number, number], end: [9, 0] as [number, number], down: 1024, up: 1024 }];
    render(ScheduleEditor, { blocks });
    expect(screen.getByRole('alert')).toHaveTextContent('End time must be different from the start time.');
  });

  it('schedule editor flags a block that becomes start == end, and Save is blocked', async () => {
    const s = base();
    s.schedule.blocks = [{ days: 1, start: [9, 0], end: [17, 0], down: 1024, up: 1024 }];
    render(SettingsPage, { settings: s, onsave: async () => {}, onclose: () => {} });
    await fireEvent.click(screen.getByRole('tab', { name: 'Limits' }));
    expect(screen.queryByRole('alert')).toBeNull();
    await fireEvent.input(screen.getByLabelText('End time'), { target: { value: '09:00' } });
    expect(screen.getByRole('alert')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Save' })).toBeDisabled();
  });

  it('drawer shows encrypted flag per peer', async () => {
    const torrent: Torrent = {
      id: 'a'.repeat(40), name: 'x', status: 'downloading', error: null, progress: 0.1,
      down_rate: 0, up_rate: 0, size: 1, peers: 2, category: null,
    };
    render(DetailDrawer, { torrent, onclose: () => {} });
    await fireEvent.click(screen.getByRole('tab', { name: 'Peers' }));
    expect(await screen.findByText('Encrypted')).toBeInTheDocument();
    expect(screen.getByText('Plaintext')).toBeInTheDocument();
  });
});
