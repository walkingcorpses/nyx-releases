<!-- DetailDrawer.svelte - the panel under the table for the selected torrent. Tabs: Files (with
     per-file progress), Peers (address, client, speeds, flags, encrypted yes/no), Trackers and a
     live speed graph. Detail data is polled while the drawer is open. -->
<script lang="ts">
  import { onDestroy, untrack } from 'svelte';
  import ProgressBar from './ProgressBar.svelte';
  import SpeedGraph from './SpeedGraph.svelte';
  import { api } from '../lib/api';
  import { formatBytes, formatPercent, formatRate } from '../lib/format';
  import type { Detail, Torrent } from '../lib/types';

  let { torrent, onclose }: { torrent: Torrent; onclose: () => void } = $props();

  type Tab = 'files' | 'peers' | 'trackers' | 'speed';
  const tabs: { id: Tab; label: string }[] = [
    { id: 'files', label: 'Files' },
    { id: 'peers', label: 'Peers' },
    { id: 'trackers', label: 'Trackers' },
    { id: 'speed', label: 'Speed' },
  ];
  let tab = $state<Tab>('files');
  let detail = $state<Detail | null>(null);
  let samples = $state<{ down: number; up: number }[]>([]);
  let lastId = '';

  /** Fetches files/peers/trackers; errors (torrent just removed) leave the old data. */
  async function load(id: string): Promise<void> {
    try {
      detail = await api.getDetail(id);
    } catch {
      /* ignore: next poll will retry or the torrent is gone */
    }
  }

  // The effect depends on the id only: `torrent` is replaced on every update, and restarting the
  // timers each time would stop them ever firing.
  const id = $derived(torrent.id);

  // Reload detail now and every 1.5 s; sample the speed once a second. Restarts per torrent.
  $effect(() => {
    const current = id;
    if (current !== lastId) {
      lastId = current;
      detail = null;
      samples = [];
    }
    void load(current);
    const poll = setInterval(() => load(current), 1500);
    const sampler = setInterval(() => {
      // untrack: read the newest rates without making them a dependency of this effect.
      const { down_rate, up_rate } = untrack(() => torrent);
      samples = [...samples.slice(-119), { down: down_rate, up: up_rate }];
    }, 1000);
    return () => {
      clearInterval(poll);
      clearInterval(sampler);
    };
  });

  onDestroy(() => (detail = null));
</script>

<section class="drawer glass" aria-label="Torrent details">
  <header>
    <div class="tabs" role="tablist">
      {#each tabs as t}
        <button role="tab" aria-selected={tab === t.id} class:on={tab === t.id} onclick={() => (tab = t.id)}>{t.label}</button>
      {/each}
    </div>
    <span class="name" title={torrent.name}>{torrent.name}</span>
    <button class="x" aria-label="Close details" onclick={onclose}>✕</button>
  </header>

  <div class="body">
    {#if tab === 'files'}
      <table>
        <thead><tr><th>File</th><th class="r">Size</th><th>Progress</th><th class="r">Priority</th></tr></thead>
        <tbody>
          {#each detail?.files ?? [] as f}
            <tr class:skipped={f.priority === 0}>
              <td class="path" title={f.path}>{f.path}</td>
              <td class="r num muted">{formatBytes(f.size)}</td>
              <td class="prog"><ProgressBar value={f.progress} /><span class="num muted">{formatPercent(f.progress)}</span></td>
              <td class="r">{f.priority === 0 ? 'Skipped' : f.priority >= 6 ? 'High' : 'Normal'}</td>
            </tr>
          {:else}
            <tr><td colspan="4" class="muted">No file information yet.</td></tr>
          {/each}
        </tbody>
      </table>
    {:else if tab === 'peers'}
      <table>
        <thead><tr><th>Address</th><th>Client</th><th class="r">Down</th><th class="r">Up</th><th>Flags</th><th>Encryption</th></tr></thead>
        <tbody>
          {#each detail?.peers ?? [] as p}
            <tr>
              <td class="num">{p.addr}</td>
              <td>{p.client || '—'}</td>
              <td class="r num">{formatRate(p.down_rate)}</td>
              <td class="r num">{formatRate(p.up_rate)}</td>
              <td class="mono">{p.flags}</td>
              <td><span class="enc" class:plain={!p.encrypted}>{p.encrypted ? 'Encrypted' : 'Plaintext'}</span></td>
            </tr>
          {:else}
            <tr><td colspan="6" class="muted">No connected peers.</td></tr>
          {/each}
        </tbody>
      </table>
    {:else if tab === 'trackers'}
      <table>
        <thead><tr><th>URL</th><th>Status</th></tr></thead>
        <tbody>
          {#each detail?.trackers ?? [] as t}
            <tr><td class="path" title={t.url}>{t.url}</td><td class:bad={t.status.startsWith('Error')}>{t.status}</td></tr>
          {:else}
            <tr><td colspan="2" class="muted">No trackers (peers come from DHT and peer exchange).</td></tr>
          {/each}
        </tbody>
      </table>
    {:else}
      <SpeedGraph {samples} />
    {/if}
  </div>
</section>

<style>
  .drawer { height: 260px; flex: none; display: flex; flex-direction: column; overflow: hidden; }
  header { display: flex; align-items: center; gap: 14px; padding: 0 8px 0 10px; height: 38px; border-bottom: 1px solid var(--line); flex: none; }
  .tabs { display: flex; gap: 2px; }
  .tabs button { background: none; border: 0; padding: 6px 12px; color: var(--text-dim); cursor: pointer; border-bottom: 2px solid transparent; }
  .tabs button.on { color: var(--text); border-bottom-color: var(--cyan); }
  .name { flex: 1; text-align: right; color: var(--text-faint); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .x { background: none; border: 0; color: var(--text-dim); cursor: pointer; width: 26px; height: 26px; border-radius: 6px; }
  .x:hover { background: var(--glass-strong); color: var(--text); }
  .body { flex: 1; overflow: auto; padding: 4px 6px; }
  table { width: 100%; border-collapse: collapse; }
  th { text-align: left; font-weight: 500; color: var(--text-faint); font-size: 11px; text-transform: uppercase; letter-spacing: 0.06em; padding: 6px 8px; position: sticky; top: 0; background: #0d0b16; }
  td { padding: 5px 8px; border-top: 1px solid rgba(255, 255, 255, 0.04); }
  .r { text-align: right; }
  .path { max-width: 380px; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .prog { width: 200px; display: grid; gap: 2px; }
  .prog span { font-size: 11px; }
  .skipped { opacity: 0.5; }
  .mono { font-family: var(--mono); font-size: 12px; }
  .enc { padding: 1px 8px; border-radius: 99px; font-size: 11px; background: rgba(74, 222, 128, 0.14); color: var(--good); }
  .enc.plain { background: rgba(251, 191, 36, 0.14); color: var(--warn); }
  .bad { color: var(--bad); }
</style>
