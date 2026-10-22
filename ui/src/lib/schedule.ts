// schedule.ts - validation and formatting helpers for the weekly bandwidth schedule editor.

import type { Block } from './types';

/** Day letters, Monday first; bit i of Block.days is day i. */
export const DAYS = ['M', 'T', 'W', 'T', 'F', 'S', 'S'];
export const DAY_NAMES = ['Monday', 'Tuesday', 'Wednesday', 'Thursday', 'Friday', 'Saturday', 'Sunday'];

/** Why a block cannot be saved, or null when it is fine. */
export function blockError(b: Block): string | null {
  if (b.start[0] === b.end[0] && b.start[1] === b.end[1]) {
    return 'End time must be different from the start time.';
  }
  if (b.days === 0) return 'Pick at least one day.';
  return null;
}

/** [9, 5] -> "09:05" (the value format of <input type="time">). */
export function toTime(t: [number, number]): string {
  return `${String(t[0]).padStart(2, '0')}:${String(t[1]).padStart(2, '0')}`;
}

/** "09:05" -> [9, 5]; null for an empty or malformed value. */
export function fromTime(s: string): [number, number] | null {
  const m = /^(\d{1,2}):(\d{2})$/.exec(s);
  if (!m) return null;
  const h = Number(m[1]);
  const min = Number(m[2]);
  return h < 24 && min < 60 ? [h, min] : null;
}
