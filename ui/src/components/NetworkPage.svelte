<!-- NetworkPage.svelte - the Network tab of the settings dialog: VPN profiles (import, rename, delete,
     test), which one is active, the network lock, timed rotation between profiles, and a live status
     card. Profile changes happen at once (they are not part of the Save draft); the active profile,
     lock and rotation edit the draft that the dialog saves. Keys never reach this page: the backend
     only sends names and endpoints. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '../lib/api';
  import { formatBytes } from '../lib/format';
  import { formatSeconds, rotationError, routeText } from '../lib/network';
  import type { NetworkStatus, Profile, Settings } from '../lib/types';

  let { draft = $bindable() }: { draft: Settings } = $props();

  let profiles = $state<Profile[]>([]);
  let status = $state<NetworkStatus | null>(null);
  let edits = $state<Record<string, { name: string; group: string }>>({});
  let importName = $state('');
  let importGroup = $state('');
  let busy = $state(false);
  let error = $state('');
  let note = $state('');
  let tested = $state<Record<string, string>>({});

  const rotErr = $derived(rotationError(draft.network.rotation));

  /** Reloads the profile list and resets the inline name/group editors to the saved values. */
  async function refresh(): Promise<void> {
    profiles = await api.listProfiles();
    edits = Object.fromEntries(profiles.map((p) => [p.id, { name: p.name, group: p.group }]));
  }

  /** Runs an action with the busy flag and error line handled in one place. */
  async function run(action: () => Promise<void>): Promise<void> {
    busy = true;
    error = '';
    note = '';
    try {
      await action();
    } catch (e) {
      error = String(e);
    }
    busy = false;
  }

  /** Picks a WireGuard .conf and imports it under the typed name (the file name when empty). */
  function importConf(): Promise<void> {
    return run(async () => {
      const path = await api.pickFile();
      if (!path) return;
      const base = path.split(/[\\/]/).pop()?.replace(/\.conf$/i, '') ?? 'VPN';
      const p = await api.importProfile(path, importName.trim() || base, importGroup.trim());
      importName = '';
      note = `Imported "${p.name}".`;
      await refresh();
    });
  }

  /** Saves the edited name and group of one profile. */
  function saveEdit(p: Profile): Promise<void> {
    return run(async () => {
      const e = edits[p.id];
      await api.renameProfile(p.id, e.name.trim(), e.group.trim());
      await refresh();
    });
  }

  /** Deletes a profile (the backend refuses the active one or one in the rotation pool). */
  function remove(p: Profile): Promise<void> {
    return run(async () => {
      await api.deleteProfile(p.id);
      await refresh();
    });
  }

  /** Brings the profile's tunnel up once and shows what the VPN server sees (can take ~35 s). */
  function test(p: Profile): Promise<void> {
    return run(async () => {
      tested[p.id] = 'Testing…';
      const r = await api.testProfile(p.id);
      tested[p.id] = r.route === 'up' ? `Works. Exit IP ${r.exit_ip ?? 'unknown'}.` : `Did not connect (${routeText(r.route)}).`;
    });
  }

  /** Adds or removes a profile from the rotation pool. */
  function togglePool(id: string, on: boolean): void {
    const pool = draft.network.rotation.pool.filter((x) => x !== id);
    draft.network.rotation.pool = on ? [...pool, id] : pool;
  }

  /** Switches to another pool profile now. */
  function rotate(): Promise<void> {
    return run(async () => {
      await api.rotateNow();
      note = 'Switched.';
    });
  }

  onMount(() => {
    void refresh();
    const poll = async () => {
      try {
        status = await api.getNetworkStatus();
      } catch {
        status = null;
      }
    };
    void poll();
    const t = setInterval(poll, 2000);
    return () => clearInterval(t);
  });
</script>

<h3>Status</h3>
<div class="card" aria-label="VPN status">
  {#if status}
    <div class="route" class:ok={status.route === 'up'} class:bad={status.route !== 'up'}>{routeText(status.route)}</div>
    <dl>
      <dt>Profile</dt><dd>{status.profile ?? 'none'}</dd>
      <dt>Last contact</dt><dd>{status.handshake_age_secs === null ? 'never' : formatSeconds(status.handshake_age_secs) + ' ago'}</dd>
      <dt>Traffic</dt><dd class="num">↓ {formatBytes(status.rx_bytes)} · ↑ {formatBytes(status.tx_bytes)}</dd>
      {#if status.next_rotation_secs !== null}<dt>Next switch</dt><dd>in {formatSeconds(status.next_rotation_secs)}</dd>{/if}
    </dl>
  {:else}
    <span class="muted">Status unavailable.</span>
  {/if}
</div>

<h3>Profiles</h3>
<p class="muted small">Import a WireGuard config (.conf) from your VPN provider. The key is stored encrypted on this PC and is never shown again.</p>
<div class="row">
  <input class="field" type="text" placeholder="Name (optional)" aria-label="New profile name" bind:value={importName} />
  <input class="field" type="text" placeholder="Provider (optional)" aria-label="New profile provider" bind:value={importGroup} />
  <button class="btn primary" disabled={busy} onclick={importConf}>Import .conf…</button>
</div>

<label class="radio">
  <input type="radio" name="active" checked={draft.network.active_profile === null} onchange={() => (draft.network.active_profile = null)} />
  <span><b>No VPN</b> <span class="muted">: connect directly (or through the proxy above)</span></span>
</label>
{#each profiles as p (p.id)}
  <div class="profile">
    <label class="radio">
      <input type="radio" name="active" aria-label={'Use ' + p.name} checked={draft.network.active_profile === p.id} onchange={() => (draft.network.active_profile = p.id)} />
      <span class="muted small">{p.endpoint}</span>
    </label>
    {#if edits[p.id]}
      <input class="field" type="text" aria-label={'Name of ' + p.name} bind:value={edits[p.id].name} />
      <input class="field" type="text" aria-label={'Provider of ' + p.name} bind:value={edits[p.id].group} />
    {/if}
    <button class="btn" disabled={busy} onclick={() => saveEdit(p)}>Rename</button>
    <button class="btn" disabled={busy} onclick={() => test(p)}>Test</button>
    <button class="btn danger" disabled={busy} onclick={() => remove(p)}>Delete</button>
    {#if tested[p.id]}<span class="small muted">{tested[p.id]}</span>{/if}
  </div>
{/each}
{#if profiles.length === 0}<p class="muted small">No profiles yet.</p>{/if}
{#if error}<p class="error-text" role="alert">{error}</p>{/if}
{#if note}<p class="muted small" role="status">{note}</p>{/if}

<h3>Network lock</h3>
<label class="radio">
  <input type="checkbox" bind:checked={draft.kill_switch} />
  <span><b>Network lock</b> <span class="muted">: pause every torrent and block web requests while the VPN is not connected</span></span>
</label>
{#if !draft.kill_switch}
  <p class="warn" role="alert">The lock is off. Torrents keep trying while the VPN is down. They still never connect around it, but nothing stops them from resuming the moment it comes back half-working.</p>
{/if}

<h3>Rotation</h3>
<label class="radio">
  <input type="checkbox" bind:checked={draft.network.rotation.enabled} aria-label="Rotate between profiles" />
  <span><b>Rotate between profiles</b> <span class="muted">: switch to another VPN server every few minutes</span></span>
</label>
{#each profiles as p (p.id)}
  <label class="radio pool">
    <input type="checkbox" aria-label={'Rotate through ' + p.name} checked={draft.network.rotation.pool.includes(p.id)} onchange={(e) => togglePool(p.id, e.currentTarget.checked)} />
    <span>{p.name} <span class="muted small">{p.group}</span></span>
  </label>
{/each}
<div class="row">
  <label class="inline">Every <input class="field num-in" type="number" min="1" aria-label="Shortest minutes" bind:value={draft.network.rotation.min_minutes} /></label>
  <label class="inline">to <input class="field num-in" type="number" min="1" aria-label="Longest minutes" bind:value={draft.network.rotation.max_minutes} /> minutes (random)</label>
  <button class="btn" disabled={busy || !draft.network.rotation.enabled} onclick={rotate}>Rotate now</button>
</div>
{#if rotErr}<p class="error-text" role="alert">{rotErr}</p>{/if}
{#if draft.network.rotation.enabled}
  <p class="warn" role="note">Each switch resets your peer connections and shows trackers a new IP address. Some private trackers ban for that. Leave rotation off for them.</p>
{/if}

<style>
  h3 { margin: 10px 0 0; font-size: 12px; text-transform: uppercase; letter-spacing: 0.08em; color: var(--text-faint); font-weight: 600; }
  .card { display: flex; gap: 18px; align-items: center; padding: 10px 12px; border: 1px solid var(--line); border-radius: var(--radius-sm); background: var(--glass); }
  .route { font-weight: 600; min-width: 120px; }
  .route.ok { color: var(--cyan); }
  .route.bad { color: var(--bad); }
  dl { display: grid; grid-template-columns: auto 1fr; gap: 2px 12px; margin: 0; font-size: 12.5px; }
  dt { color: var(--text-dim); }
  dd { margin: 0; }
  .row { display: flex; gap: 8px; align-items: center; flex-wrap: wrap; }
  .profile { display: grid; grid-template-columns: 170px 1fr 1fr auto auto auto; gap: 6px; align-items: center; }
  .profile .small { grid-column: 1 / -1; }
  .radio { display: flex; align-items: flex-start; gap: 10px; cursor: pointer; }
  .radio input { margin-top: 3px; accent-color: var(--violet); }
  .pool { margin-left: 24px; }
  .inline { display: flex; align-items: center; gap: 6px; color: var(--text-dim); }
  .num-in { width: 70px; }
  .small { font-size: 12px; margin: 0; }
  .warn { margin: 4px 0; padding: 8px 12px; border-radius: var(--radius-sm); background: rgba(251, 191, 36, 0.1); border: 1px solid rgba(251, 191, 36, 0.4); color: #fde7a6; font-size: 12px; }
</style>
