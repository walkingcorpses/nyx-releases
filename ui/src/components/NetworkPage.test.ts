// NetworkPage.test.ts - the Network tab: profiles without any key material, importing through the
// file picker, the lock warning, the rotation rules and warning, and the title-bar route chip.
import { fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import NetworkPage from './NetworkPage.testhost.svelte';
import TitleBar from './TitleBar.svelte';
import { rotationError } from '../lib/network';
import { appStatus } from '../lib/store';
import type { AppStatus, Settings } from '../lib/types';

const h = vi.hoisted(() => ({
  api: {
    listProfiles: vi.fn(),
    importProfile: vi.fn(),
    renameProfile: vi.fn(async () => {}),
    deleteProfile: vi.fn(async () => {}),
    testProfile: vi.fn(),
    getNetworkStatus: vi.fn(),
    rotateNow: vi.fn(async () => 'p2'),
    pickFile: vi.fn(async () => 'D:\\vpn\\amsterdam.conf'),
    windowMinimize: vi.fn(),
    windowToggleMaximize: vi.fn(),
    windowClose: vi.fn(),
  },
}));
vi.mock('../lib/api', () => ({ api: h.api, inTauri: false }));

const PROFILES = [
  { id: 'p1', name: 'Amsterdam', group: 'Mullvad', endpoint: '193.32.126.66:51820', active: true },
  { id: 'p2', name: 'Zurich', group: 'Mullvad', endpoint: '185.65.134.80:51820', active: false },
];

/** Settings as the backend sends them (profiles never included). */
function base(): Settings {
  return {
    encryption: 'Forced', proxy: null, max_active_downloads: 3, max_active_seeds: 3,
    down_limit: 0, up_limit: 0, schedule: { blocks: [] }, categories: [],
    default_save_dir: 'D:\\Downloads', start_minimized: false, minimize_to_tray: true,
    kill_switch: true, bind_interface: null, blocklist: null, check_updates: false,
    rss: { feeds: [], interval_minutes: 30 }, search_url: '',
    network: { active_profile: 'p1', rotation: { enabled: false, pool: [], min_minutes: 5, max_minutes: 10 } },
  };
}

/** A status with defaults; `over` changes fields. */
function status(over: Partial<AppStatus> = {}): AppStatus {
  return { version: '1.0.0', proxy_down: false, killed: false, update: null, warnings: [], route: 'up', profile_name: 'Amsterdam', ...over };
}

beforeEach(() => {
  vi.clearAllMocks();
  h.api.listProfiles.mockResolvedValue(PROFILES);
  h.api.getNetworkStatus.mockResolvedValue({
    route: 'up', profile: 'Amsterdam', handshake_age_secs: 5, rx_bytes: 1024, tx_bytes: 512, exit_ip: null, next_rotation_secs: null,
  });
  h.api.importProfile.mockResolvedValue({ id: 'p3', name: 'amsterdam', group: '', endpoint: '1.2.3.4:51820', active: false });
  h.api.testProfile.mockResolvedValue({
    route: 'up', profile: 'Amsterdam', handshake_age_secs: 1, rx_bytes: 0, tx_bytes: 0, exit_ip: '203.0.113.7', next_rotation_secs: null,
  });
  appStatus.set(null);
});

describe('network page', () => {
  it('lists profiles without any key material', async () => {
    const { container } = render(NetworkPage, { initial: base() });
    expect(await screen.findByLabelText('Name of Amsterdam')).toHaveValue('Amsterdam');
    expect(screen.getByText('193.32.126.66:51820')).toBeInTheDocument();
    // The page only ever gets names and endpoints; nothing key-like may be on screen or in an input.
    expect(container.innerHTML).not.toMatch(/PrivateKey|sealed|dpapi|plain:/i);
  });

  it('import button calls import_profile with the chosen path', async () => {
    render(NetworkPage, { initial: base() });
    await screen.findByLabelText('Name of Amsterdam');
    await fireEvent.input(screen.getByLabelText('New profile provider'), { target: { value: 'Mullvad' } });
    await fireEvent.click(screen.getByRole('button', { name: /Import \.conf/ }));
    await waitFor(() => expect(h.api.importProfile).toHaveBeenCalledWith('D:\\vpn\\amsterdam.conf', 'amsterdam', 'Mullvad'));
    expect(await screen.findByText(/Imported "amsterdam"/)).toBeInTheDocument();
  });

  it('test button shows the exit IP', async () => {
    render(NetworkPage, { initial: base() });
    await screen.findByLabelText('Name of Amsterdam');
    await fireEvent.click(screen.getAllByRole('button', { name: 'Test' })[0]);
    expect(await screen.findByText(/Exit IP 203\.0\.113\.7/)).toBeInTheDocument();
  });

  it('lock toggle off shows a warning', async () => {
    render(NetworkPage, { initial: base() });
    await screen.findByLabelText('Name of Amsterdam');
    expect(screen.queryByText(/The lock is off/)).toBeNull();
    await fireEvent.click(screen.getByRole('checkbox', { name: /Network lock/ }));
    expect(screen.getByRole('alert')).toHaveTextContent(/The lock is off/);
  });

  it('rotation needs two selected profiles to enable', async () => {
    const draft = base();
    render(NetworkPage, { initial: draft });
    await screen.findByLabelText('Name of Amsterdam');
    await fireEvent.click(screen.getByRole('checkbox', { name: 'Rotate between profiles' }));
    expect(screen.getByRole('alert')).toHaveTextContent(/at least two profiles/);
    await fireEvent.click(screen.getByRole('checkbox', { name: 'Rotate through Amsterdam' }));
    await fireEvent.click(screen.getByRole('checkbox', { name: 'Rotate through Zurich' }));
    expect(screen.queryByText(/at least two profiles/)).toBeNull();
  });

  it('private tracker warning shown when rotation is on', async () => {
    const draft = base();
    draft.network.rotation.enabled = true;
    draft.network.rotation.pool = ['p1', 'p2'];
    render(NetworkPage, { initial: draft });
    await screen.findByLabelText('Name of Amsterdam');
    expect(screen.getByText(/Some private trackers ban for that/)).toBeInTheDocument();
  });
});

describe('rotation rules', () => {
  const rot = { enabled: false, pool: [] as string[], min_minutes: 5, max_minutes: 10 };
  it('mirrors the backend checks', () => {
    expect(rotationError(rot)).toBe('');
    expect(rotationError({ ...rot, min_minutes: 0 })).toMatch(/at least 1 minute/);
    expect(rotationError({ ...rot, min_minutes: 11 })).toMatch(/cannot be longer/);
    expect(rotationError({ ...rot, enabled: true, pool: ['a'] })).toMatch(/at least two/);
    expect(rotationError({ ...rot, enabled: true, pool: ['a', 'b'] })).toBe('');
  });
});

describe('title bar route chip', () => {
  it('is red while the lock is holding', () => {
    appStatus.set(status({ killed: true, route: 'stale' }));
    render(TitleBar);
    const chip = screen.getByRole('status', { name: 'VPN route' });
    expect(chip).toHaveTextContent('Locked: Connection lost');
    expect(chip.className).toContain('bad');
  });

  it('is calm when connected and hidden without a VPN', () => {
    appStatus.set(status());
    const { unmount } = render(TitleBar);
    const chip = screen.getByRole('status', { name: 'VPN route' });
    expect(chip).toHaveTextContent('VPN: Connected');
    expect(chip.className).not.toContain('bad');
    unmount();
    appStatus.set(status({ route: 'none', profile_name: null }));
    render(TitleBar);
    expect(screen.queryByRole('status', { name: 'VPN route' })).toBeNull();
  });
});
