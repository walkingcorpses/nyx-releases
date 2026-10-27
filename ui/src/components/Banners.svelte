<!-- Banners.svelte - the strips under the toolbar for things that need attention: the proxy kill
     switch, a newer Nyx, and startup warnings. State comes from the backend status (polled by App). -->
<script lang="ts">
  import { api } from '../lib/api';
  import { routeText } from '../lib/network';
  import { appStatus } from '../lib/store';

  let installing = $state(false);
  let installed = $state(false);
  let error = $state('');
  let dismissed = $state(new Set<string>());

  /** Downloads, verifies and installs the update, then offers a restart. */
  async function install(): Promise<void> {
    installing = true;
    error = '';
    try {
      await api.installUpdate();
      installed = true;
    } catch (e) {
      error = String(e);
    }
    installing = false;
  }

  function dismiss(key: string): void {
    dismissed = new Set([...dismissed, key]);
  }
</script>

{#if $appStatus}
  {#if $appStatus.killed && $appStatus.route !== 'none'}
    <div class="banner bad" role="alert">
      <b>VPN {routeText($appStatus.route).toLowerCase()}.</b> Every torrent is paused and web requests are blocked until it is back, so nothing can connect around it.
    </div>
  {:else if $appStatus.killed}
    <div class="banner bad" role="alert">
      <b>Proxy unreachable.</b> Every torrent is paused until it is back, so nothing can connect around it.
    </div>
  {:else if $appStatus.proxy_down}
    <div class="banner warn" role="alert">
      <b>Proxy unreachable.</b> The kill switch is off, so torrents keep trying, but they never connect directly.
    </div>
  {/if}

  {#if $appStatus.update && !dismissed.has('update')}
    <div class="banner info" role="status">
      {#if installed}
        <span><b>Nyx {$appStatus.update.version}</b> is installed. Restart to use it.</span>
        <button class="btn primary" onclick={() => api.restartApp()}>Restart now</button>
      {:else}
        <span><b>Nyx {$appStatus.update.version}</b> is available.</span>
        <button class="btn primary" disabled={installing} onclick={install}>{installing ? 'Installing…' : 'Update now'}</button>
        <button class="btn" onclick={() => dismiss('update')}>Later</button>
      {/if}
      {#if error}<span class="error-text">{error}</span>{/if}
    </div>
  {/if}

  {#each $appStatus.warnings.filter((w) => !dismissed.has(w)) as w (w)}
    <div class="banner warn" role="status">
      <span>{w}</span>
      <button class="btn icon" aria-label="Dismiss" onclick={() => dismiss(w)}>✕</button>
    </div>
  {/each}
{/if}

<style>
  .banner { display: flex; align-items: center; gap: 12px; margin: 0 12px 8px; padding: 8px 12px; border-radius: var(--radius-sm); border: 1px solid; font-size: 12.5px; }
  .banner span { flex: 1; }
  .bad { background: rgba(251, 113, 133, 0.12); border-color: rgba(251, 113, 133, 0.45); color: #ffc2cb; }
  .warn { background: rgba(251, 191, 36, 0.1); border-color: rgba(251, 191, 36, 0.4); color: #fde7a6; }
  .info { background: var(--violet-soft); border-color: rgba(139, 92, 246, 0.45); color: #ddd0ff; }
  .banner .btn { height: 26px; }
</style>
