<!-- ScheduleEditor.svelte - the weekly bandwidth schedule: time blocks that override the global
     speed limits. Each block has day toggles, a start and end time (end earlier than start means
     it runs overnight), and its own down/up caps in KB/s (0 = unlimited). -->
<script lang="ts">
  import { blockError, DAYS, DAY_NAMES, fromTime, toTime } from '../lib/schedule';
  import type { Block } from '../lib/types';

  let { blocks = $bindable() }: { blocks: Block[] } = $props();

  /** A new block: weekdays 09:00-17:00 at 500 KB/s down, 100 KB/s up. */
  function add(): void {
    blocks = [...blocks, { days: 0b0011111, start: [9, 0], end: [17, 0], down: 500 * 1024, up: 100 * 1024 }];
  }

  function remove(i: number): void {
    blocks = blocks.filter((_, j) => j !== i);
  }

  function toggleDay(i: number, d: number): void {
    blocks[i].days ^= 1 << d;
  }

  /** Time inputs report "HH:MM"; ignore a cleared field so the block is never left half-set. */
  function setTime(i: number, which: 'start' | 'end', value: string): void {
    const t = fromTime(value);
    if (t) blocks[i][which] = t;
  }

  /** KB/s field -> bytes/s (empty or negative becomes 0 = unlimited). */
  function setRate(i: number, which: 'down' | 'up', value: string): void {
    blocks[i][which] = Math.max(0, Math.round(Number(value) || 0)) * 1024;
  }
</script>

<div class="sched">
  {#each blocks as b, i}
    {@const err = blockError(b)}
    <div class="block glass" class:bad={err}>
      <div class="days" role="group" aria-label="Days">
        {#each DAYS as d, k}
          <button class:on={b.days & (1 << k)} aria-label={DAY_NAMES[k]} aria-pressed={!!(b.days & (1 << k))} onclick={() => toggleDay(i, k)}>{d}</button>
        {/each}
      </div>
      <label>From <input class="field time" type="time" value={toTime(b.start)} oninput={(e) => setTime(i, 'start', e.currentTarget.value)} aria-label="Start time" /></label>
      <label>To <input class="field time" type="time" value={toTime(b.end)} oninput={(e) => setTime(i, 'end', e.currentTarget.value)} aria-label="End time" /></label>
      <label>Down <input class="field rate" type="number" min="0" value={Math.round(b.down / 1024)} oninput={(e) => setRate(i, 'down', e.currentTarget.value)} aria-label="Download limit KB/s" /> <span class="muted">KB/s</span></label>
      <label>Up <input class="field rate" type="number" min="0" value={Math.round(b.up / 1024)} oninput={(e) => setRate(i, 'up', e.currentTarget.value)} aria-label="Upload limit KB/s" /> <span class="muted">KB/s</span></label>
      <button class="btn icon danger" aria-label="Remove block" onclick={() => remove(i)}>✕</button>
      {#if err}<p class="error-text full" role="alert">{err}</p>{/if}
    </div>
  {:else}
    <p class="muted">No schedule. The global limits apply all the time.</p>
  {/each}
  <button class="btn" onclick={add}>Add time block</button>
</div>

<style>
  .sched { display: grid; gap: 8px; }
  .block { display: flex; flex-wrap: wrap; align-items: center; gap: 10px; padding: 8px 10px; }
  .block.bad { border-color: rgba(251, 113, 133, 0.5); }
  .days { display: flex; gap: 3px; }
  .days button { width: 24px; height: 24px; border-radius: 50%; border: 1px solid var(--line); background: none; color: var(--text-dim); cursor: pointer; font-size: 11px; }
  .days button.on { background: var(--violet-soft); border-color: var(--violet); color: #d6c8ff; }
  label { display: flex; align-items: center; gap: 6px; color: var(--text-dim); }
  .time { width: 96px; }
  .rate { width: 80px; }
  .full { flex-basis: 100%; margin: 0; }
</style>
