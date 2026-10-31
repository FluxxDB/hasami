import { describe, expect, it } from 'vitest';
import { formatBytes, formatEta, formatFps, formatPercent, progressLine } from './format';

describe('formatBytes', () => {
  it('uses decimal units and drops decimals at three digits', () => {
    expect(formatBytes(412_000_000)).toBe('412 MB');
    expect(formatBytes(19_400_000)).toBe('19.4 MB');
  });
  it('drops a trailing .0', () => {
    expect(formatBytes(20_000_000)).toBe('20 MB');
  });
  it('covers bytes, KB and GB', () => {
    expect(formatBytes(0)).toBe('0 B');
    expect(formatBytes(999)).toBe('999 B');
    expect(formatBytes(1_500)).toBe('1.5 KB');
    expect(formatBytes(2_400_000_000)).toBe('2.4 GB');
  });
});

describe('formatEta', () => {
  it('formats m:ss', () => {
    expect(formatEta(24)).toBe('0:24');
    expect(formatEta(125)).toBe('2:05');
  });
  it('rounds up so it never shows 0:00 while work remains', () => {
    expect(formatEta(24.2)).toBe('0:25');
    expect(formatEta(0)).toBe('0:00');
  });
  it('adds hours past an hour', () => {
    expect(formatEta(3725)).toBe('1:02:05');
  });
});

describe('formatPercent', () => {
  it('floors to a whole percent and clamps', () => {
    expect(formatPercent(0.384)).toBe('38%');
    expect(formatPercent(0.999)).toBe('99%');
    expect(formatPercent(1)).toBe('100%');
    expect(formatPercent(-0.2)).toBe('0%');
    expect(formatPercent(1.4)).toBe('100%');
  });
});

describe('progressLine', () => {
  it('shows pass, percent and eta', () => {
    expect(progressLine({ pass: 1, passes: 2, fraction: 0.384, etaS: 24 })).toBe(
      'Pass 1 of 2 · 38% · about 0:24 left',
    );
  });
  it('omits the pass for single-pass jobs and the eta when unknown', () => {
    expect(progressLine({ pass: 1, passes: 1, fraction: 0.5, etaS: null })).toBe('50%');
  });
});

describe('formatFps', () => {
  it('keeps at most two decimals and drops trailing zeros', () => {
    expect(formatFps(29.97002997)).toBe('29.97');
    expect(formatFps(60)).toBe('60');
    expect(formatFps(23.976)).toBe('23.98');
    expect(formatFps(12.5)).toBe('12.5');
  });
});
