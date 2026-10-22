// format.ts - turns raw numbers into the short strings the table shows, plus magnet helpers.

const UNITS = ['B', 'KB', 'MB', 'GB', 'TB'];

/** 1536 -> "1.5 KB". Bytes are whole numbers; larger units get one decimal below 100, none above. */
export function formatBytes(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return '0 B';
  let i = 0;
  while (n >= 1024 && i < UNITS.length - 1) {
    n /= 1024;
    i++;
  }
  if (i === 0) return `${Math.round(n)} B`;
  return `${n >= 100 ? Math.round(n) : n.toFixed(1)} ${UNITS[i]}`;
}

/** Bytes per second: 0 -> "0 B/s", 2048 -> "2.0 KB/s". */
export function formatRate(n: number): string {
  return `${formatBytes(n)}/s`;
}

/** Seconds remaining as "1h 2m". Infinity (or negative / NaN) is "∞". */
export function formatEta(seconds: number): string {
  if (!Number.isFinite(seconds) || seconds < 0) return '∞';
  const s = Math.round(seconds);
  const d = Math.floor(s / 86400);
  const h = Math.floor((s % 86400) / 3600);
  const m = Math.floor((s % 3600) / 60);
  if (d > 0) return `${d}d ${h}h`;
  if (h > 0) return `${h}h ${m}m`;
  if (m > 0) return `${m}m ${s % 60}s`;
  return `${s}s`;
}

/** 0.4567 -> "45.7%". */
export function formatPercent(p: number): string {
  return `${(Math.max(0, Math.min(1, p)) * 100).toFixed(1)}%`;
}

/** Display name of a magnet link: its `dn` parameter, else a short form of the hash. */
export function magnetName(uri: string): string {
  const q = uri.indexOf('?');
  const params = new URLSearchParams(q >= 0 ? uri.slice(q + 1) : '');
  const dn = params.get('dn');
  if (dn) return dn;
  const xt = params.get('xt') ?? '';
  const hash = xt.split(':').pop() ?? '';
  return hash ? `Magnet ${hash.slice(0, 8)}…` : 'Magnet link';
}

/** True for text that looks like a magnet link (what Ctrl+V should react to). */
export function isMagnet(text: string): boolean {
  return /^\s*magnet:\?/i.test(text);
}
