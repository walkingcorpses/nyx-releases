<!-- App.svelte - the window layout: title bar, toolbar, torrent table, detail drawer, and the
     two modal layers (add dialog, settings). Loads the starting data and follows backend events. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import AddHost from './components/AddHost.svelte';
  import DetailDrawer from './components/DetailDrawer.svelte';
  import SettingsPage from './components/SettingsPage.svelte';
  import TitleBar from './components/TitleBar.svelte';
  import Toolbar from './components/Toolbar.svelte';
  import TorrentTable from './components/TorrentTable.svelte';
  import { api } from './lib/api';
  import { applyUpdate, selected, selectedId, setAll, settings, settingsOpen } from './lib/store';
  import type { Settings } from './lib/types';

  let loadError = $state('');

  onMount(() => {
    let off: (() => void) | undefined;
    let alive = true;
    // Subscribe first, then load, so no update falls in the gap between the two.
    api.onUpdate(applyUpdate).then((f) => (alive ? (off = f) : f()));
    api.listTorrents().then(setAll).catch((e) => (loadError = String(e)));
    api.getSettings().then((s) => settings.set(s)).catch((e) => (loadError = String(e)));
    return () => {
      alive = false;
      off?.();
    };
  });

  /** Saves settings through the backend, then mirrors them into the store. */
  async function saveSettings(s: Settings): Promise<void> {
    await api.setSettings(s);
    settings.set(s);
  }
</script>

<div class="app">
  <TitleBar />
  <main>
    <Toolbar />
    {#if loadError}<p class="error-text banner">{loadError}</p>{/if}
    <TorrentTable />
    {#if $selected}
      <DetailDrawer torrent={$selected} onclose={() => selectedId.set(null)} />
    {/if}
  </main>
  <AddHost />
  {#if $settingsOpen && $settings}
    <SettingsPage settings={$settings} onsave={saveSettings} onclose={() => settingsOpen.set(false)} />
  {/if}
</div>

<style>
  .app { display: flex; flex-direction: column; height: 100%; }
  main { flex: 1; min-height: 0; display: flex; flex-direction: column; gap: 10px; padding: 0 12px 12px; }
  .banner { margin: 0 4px; }
</style>
