<!-- SpeedGraph.svelte - live speed graph drawn on a canvas from the last 120 samples
     (one per second). Download is cyan, upload is violet, both with a soft fill. -->
<script lang="ts">
  let { samples }: { samples: { down: number; up: number }[] } = $props();

  let canvas: HTMLCanvasElement | undefined = $state();
  let box: HTMLDivElement | undefined = $state();

  /** Redraws whenever the samples or the canvas change. */
  $effect(() => {
    void samples.length;
    if (!canvas || !box) return;
    const dpr = window.devicePixelRatio || 1;
    const w = box.clientWidth;
    const h = box.clientHeight;
    canvas.width = w * dpr;
    canvas.height = h * dpr;
    const ctx = canvas.getContext('2d');
    if (!ctx) return;
    ctx.scale(dpr, dpr);
    ctx.clearRect(0, 0, w, h);

    const max = Math.max(1024, ...samples.map((s) => Math.max(s.down, s.up))) * 1.15;
    const step = w / 119;
    const y = (v: number) => h - 18 - (v / max) * (h - 30);

    // Grid lines
    ctx.strokeStyle = 'rgba(255,255,255,0.06)';
    ctx.lineWidth = 1;
    for (let i = 0; i <= 3; i++) {
      const gy = 12 + ((h - 30) / 3) * i;
      ctx.beginPath();
      ctx.moveTo(0, gy);
      ctx.lineTo(w, gy);
      ctx.stroke();
    }

    /** One series: a line with a gradient fill below it. */
    const series = (pick: (s: { down: number; up: number }) => number, color: string, fill: string) => {
      if (samples.length < 2) return;
      const x0 = w - (samples.length - 1) * step;
      ctx.beginPath();
      samples.forEach((s, i) => {
        const x = x0 + i * step;
        i === 0 ? ctx.moveTo(x, y(pick(s))) : ctx.lineTo(x, y(pick(s)));
      });
      ctx.strokeStyle = color;
      ctx.lineWidth = 1.8;
      ctx.lineJoin = 'round';
      ctx.stroke();
      ctx.lineTo(w, h - 18);
      ctx.lineTo(x0, h - 18);
      ctx.closePath();
      const g = ctx.createLinearGradient(0, 0, 0, h);
      g.addColorStop(0, fill);
      g.addColorStop(1, 'rgba(0,0,0,0)');
      ctx.fillStyle = g;
      ctx.fill();
    };
    series((s) => s.up, '#8b5cf6', 'rgba(139,92,246,0.28)');
    series((s) => s.down, '#22d3ee', 'rgba(34,211,238,0.28)');
  });
</script>

<div class="graph" bind:this={box} role="img" aria-label="Speed graph">
  <canvas bind:this={canvas}></canvas>
  <div class="legend"><i class="d"></i> Download <i class="u"></i> Upload <span class="muted">last 2 minutes</span></div>
</div>

<style>
  .graph { position: relative; height: 100%; min-height: 120px; }
  canvas { position: absolute; inset: 0; width: 100%; height: 100%; }
  .legend { position: absolute; left: 8px; bottom: 0; display: flex; align-items: center; gap: 6px; font-size: 11px; color: var(--text-dim); }
  i { width: 8px; height: 8px; border-radius: 50%; display: inline-block; margin-left: 6px; }
  i.d { background: var(--cyan); }
  i.u { background: var(--violet); }
</style>
