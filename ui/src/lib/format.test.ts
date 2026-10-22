// format.test.ts - the number/string helpers.
import { describe, expect, it } from 'vitest';
import { formatBytes, formatEta, formatPercent, formatRate, isMagnet, magnetName } from './format';

describe('format', () => {
  it('formatBytes(1536) === "1.5 KB"', () => {
    expect(formatBytes(1536)).toBe('1.5 KB');
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(512)).toBe('512 B');
    expect(formatBytes(150 * 1024 * 1024)).toBe('150 MB');
  });

  it('formatRate(0) === "0 B/s"', () => {
    expect(formatRate(0)).toBe('0 B/s');
    expect(formatRate(2048)).toBe('2.0 KB/s');
  });

  it('formatEta handles infinity and units', () => {
    expect(formatEta(Infinity)).toBe('∞');
    expect(formatEta(3725)).toBe('1h 2m');
    expect(formatEta(59)).toBe('59s');
    expect(formatEta(125)).toBe('2m 5s');
    expect(formatEta(90000)).toBe('1d 1h');
  });

  it('formatPercent clamps', () => {
    expect(formatPercent(0.4567)).toBe('45.7%');
    expect(formatPercent(2)).toBe('100.0%');
  });

  it('magnetName reads dn or falls back to the hash', () => {
    expect(magnetName('magnet:?xt=urn:btih:abcdef0123456789&dn=My+Film%20%282020%29')).toBe('My Film (2020)');
    expect(magnetName('magnet:?xt=urn:btih:abcdef0123456789')).toBe('Magnet abcdef01…');
    expect(magnetName('magnet:?x=1')).toBe('Magnet link');
  });

  it('isMagnet', () => {
    expect(isMagnet('  MAGNET:?xt=urn:btih:abc')).toBe(true);
    expect(isMagnet('https://example.com')).toBe(false);
  });
});
