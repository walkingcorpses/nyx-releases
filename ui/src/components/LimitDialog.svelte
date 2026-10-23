<!-- LimitDialog.svelte - sets download/upload speed caps for one torrent (KB/s, 0 = unlimited).
     The caps are kept by the engine and survive restarts. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '../lib/api';
  import type { Torrent } from '../lib/types';

  let { torrent, onclose }: { torrent: Torrent; onclose: () => void } = $props();

  let down = $state(0);
  let up = $state(0);
  let error = $state('');

  // Start from the caps the torrent has now.
  onMount(async () => {
    try {
      const d = await api.getDetail(torrent.id);
      down = Math.round(d.down_limit / 1024);
      up = Math.round(d.up_limit / 1024);
    } catch {
      /* keep 0 = unlimited */
    }
  });

  /** Applies the caps (KB/s -> bytes/s) and closes. */
  async function apply(): Promise<void> {
    try {
      await api.setTorrentLimits(torrent.id, Math.max(0, down) * 1024, Math.max(0, up) * 1024);
      onclose();
    } catch (e) {
      error = String(e);
    }
  }
</script>

<svelte:window onkeydown={(e) => e.key === 'Escape' && onclose()} />

<div class="scrim" role="presentation">
  <div class="dialog glass" role="dialog" aria-modal="true" aria-label="Torrent speed limits">
    <h2>Speed limits</h2>
    <div class="name" title={torrent.name}>{torrent.name}</div>
    <label>Download <span class="muted">(KB/s, 0 = unlimited)</span>
      <input class="field" type="number" min="0" bind:value={down} aria-label="Torrent download limit" />
    </label>
    <label>Upload <span class="muted">(KB/s, 0 = unlimited)</span>
      <input class="field" type="number" min="0" bind:value={up} aria-label="Torrent upload limit" />
    </label>
    {#if error}<p class="error-text">{error}</p>{/if}
    <div class="actions">
      <button class="btn" onclick={onclose}>Cancel</button>
      <button class="btn primary" onclick={apply}>Apply</button>
    </div>
  </div>
</div>

<style>
  .scrim { position: fixed; inset: 36px 0 0 0; background: rgba(3, 2, 8, 0.6); display: grid; place-items: center; z-index: 55; backdrop-filter: blur(3px); }
  .dialog { width: min(380px, calc(100vw - 40px)); padding: 20px; background: var(--bg-raised); display: grid; gap: 12px; box-shadow: 0 30px 80px rgba(0, 0, 0, 0.6); }
  h2 { margin: 0; font-size: 16px; }
  .name { color: var(--text-dim); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  label { display: grid; gap: 4px; color: var(--text-dim); }
  .actions { display: flex; justify-content: flex-end; gap: 8px; }
</style>
