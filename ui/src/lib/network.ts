// network.ts - small pure helpers for the Network page: checking the rotation settings the same
// way the backend does (so Save can be disabled before a round trip) and wording for the VPN route.

import type { Rotation } from './types';

/** Why the rotation settings cannot be saved, or '' when they are fine. Mirrors the backend rules. */
export function rotationError(r: Rotation): string {
  if (r.min_minutes < 1) return 'The shortest time must be at least 1 minute.';
  if (r.min_minutes > r.max_minutes) return 'The shortest time cannot be longer than the longest.';
  if (r.enabled && r.pool.length < 2) return 'Pick at least two profiles to rotate between.';
  return '';
}

/** A short human phrase for the VPN route word the backend sends. */
export function routeText(route: string): string {
  switch (route) {
    case 'up': return 'Connected';
    case 'starting': return 'Connecting…';
    case 'stale': return 'Connection lost';
    case 'down': return 'Not connected';
    default: return 'No VPN';
  }
}

/** Seconds as "2 m 05 s" style text for ages and countdowns. */
export function formatSeconds(s: number): string {
  if (s < 60) return `${s} s`;
  const m = Math.floor(s / 60);
  return `${m} m ${String(s % 60).padStart(2, '0')} s`;
}
