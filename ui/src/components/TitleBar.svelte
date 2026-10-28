<!-- TitleBar.svelte - custom window title bar matched to the theme. The empty area is a drag
     region; the three buttons call the window API through api.ts. -->
<script lang="ts">
  import { api } from '../lib/api';
  import { formatRate } from '../lib/format';
  import { routeText } from '../lib/network';
  import { appStatus, totals } from '../lib/store';
</script>

<header class="bar" data-tauri-drag-region>
  <div class="brand" data-tauri-drag-region>
    <svg viewBox="0 0 24 24" width="16" height="16" aria-hidden="true" data-tauri-drag-region>
      <defs>
        <linearGradient id="g" x1="0" y1="0" x2="1" y2="1">
          <stop offset="0" stop-color="#8b5cf6" /><stop offset="1" stop-color="#22d3ee" />
        </linearGradient>
      </defs>
      <path d="M21 12.79A9 9 0 1 1 11.21 3 7 7 0 0 0 21 12.79z" fill="url(#g)" />
    </svg>
    <span data-tauri-drag-region>Nyx</span>
  </div>
  {#if $appStatus && $appStatus.route !== 'none'}
    <div class="route" class:bad={$appStatus.killed || $appStatus.route !== 'up'} data-tauri-drag-region role="status" aria-label="VPN route">
      <span class="dot" data-tauri-drag-region></span>{$appStatus.killed ? 'Locked: ' : 'VPN: '}{routeText($appStatus.route)}
    </div>
  {/if}
  <div class="speed num" data-tauri-drag-region aria-label="Total speed">
    <span class="d" data-tauri-drag-region>↓ {formatRate($totals.down)}</span>
    <span class="u" data-tauri-drag-region>↑ {formatRate($totals.up)}</span>
  </div>
  <div class="buttons">
    <button aria-label="Minimize" onclick={() => api.windowMinimize()}>
      <svg viewBox="0 0 10 10" width="10" height="10"><path d="M0 5h10" stroke="currentColor" /></svg>
    </button>
    <button aria-label="Maximize" onclick={() => api.windowToggleMaximize()}>
      <svg viewBox="0 0 10 10" width="10" height="10"><rect x="0.5" y="0.5" width="9" height="9" fill="none" stroke="currentColor" /></svg>
    </button>
    <button class="close" aria-label="Close" onclick={() => api.windowClose()}>
      <svg viewBox="0 0 10 10" width="10" height="10"><path d="M0 0l10 10M10 0L0 10" stroke="currentColor" /></svg>
    </button>
  </div>
</header>

<style>
  .bar {
    display: flex; align-items: center;
    height: 36px; padding-left: 14px;
    border-bottom: 1px solid var(--line);
    background: rgba(7, 6, 13, 0.7);
  }
  .brand { display: flex; align-items: center; gap: 8px; font-weight: 600; letter-spacing: 0.04em; }
  .speed { margin-left: auto; margin-right: 16px; display: flex; gap: 14px; font-size: 12px; }
  .route { display: flex; align-items: center; gap: 6px; margin-left: 18px; font-size: 12px; color: var(--cyan); }
  .route.bad { color: var(--bad); }
  .dot { width: 7px; height: 7px; border-radius: 50%; background: currentColor; }
  .d { color: var(--cyan); }
  .u { color: #b79bff; }
  .buttons { display: flex; height: 100%; }
  button {
    width: 46px; border: 0; background: transparent; color: var(--text-dim);
    display: grid; place-items: center; cursor: default; transition: background 0.12s, color 0.12s;
  }
  button:hover { background: var(--glass-strong); color: var(--text); }
  .close:hover { background: #e5384e; color: white; }
</style>
