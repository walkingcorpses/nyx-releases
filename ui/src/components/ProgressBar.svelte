<!-- ProgressBar.svelte - a torrent's progress: smooth width animation, a violet-to-cyan fill, and a
     moving sheen while it is actively downloading. `value` is 0..1. -->
<script lang="ts">
  let { value, active = false, error = false }: { value: number; active?: boolean; error?: boolean } = $props();
  const pct = $derived(Math.max(0, Math.min(1, value)) * 100);
</script>

<div class="bar" class:error role="progressbar" aria-valuemin="0" aria-valuemax="100" aria-valuenow={Math.round(pct)}>
  <div class="fill" class:active style="width: {pct}%"></div>
</div>

<style>
  .bar {
    position: relative; height: 8px; border-radius: 99px; overflow: hidden;
    background: rgba(255, 255, 255, 0.07);
  }
  .fill {
    height: 100%; border-radius: 99px;
    background: linear-gradient(90deg, var(--violet), var(--cyan));
    transition: width 0.9s cubic-bezier(0.22, 0.8, 0.3, 1);
    position: relative;
  }
  .error .fill { background: var(--bad); }
  .fill.active::after {
    content: ''; position: absolute; inset: 0;
    background: linear-gradient(100deg, transparent 30%, rgba(255, 255, 255, 0.45) 50%, transparent 70%);
    background-size: 200% 100%;
    animation: sheen 1.6s linear infinite;
  }
  @keyframes sheen { from { background-position: 200% 0; } to { background-position: -200% 0; } }
</style>
