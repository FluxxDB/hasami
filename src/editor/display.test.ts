import { describe, expect, it } from 'vitest';
import { TRACK_COLORS, fileName, formatClock, formatDb, metaLine, mixHex, trackColor } from './display';
import type { EditorMedia } from './api';

describe('formatClock', () => {
  it('formats m:ss, rounding to the nearest second', () => {
    expect(formatClock(44)).toBe('0:44');
    expect(formatClock(90)).toBe('1:30');
    expect(formatClock(43.98)).toBe('0:44');
    expect(formatClock(0)).toBe('0:00');
  });
  it('adds hours past an hour', () => {
    expect(formatClock(3725)).toBe('1:02:05');
  });
});

describe('formatDb', () => {
  it('shows one decimal with a real minus sign', () => {
    expect(formatDb(0)).toBe('0.0 dB');
    expect(formatDb(-6)).toBe('−6.0 dB');
    expect(formatDb(3.5)).toBe('+3.5 dB');
  });
  it('shows −∞ at the bottom of the range', () => {
    expect(formatDb(-60)).toBe('−∞ dB');
  });
});

describe('track colors', () => {
  it('uses the six spec colors in order and wraps after A6', () => {
    expect(TRACK_COLORS).toEqual(['#7fa58c', '#c9a063', '#7f9cc4', '#b08bb3', '#4fa8a2', '#d89a3e']);
    expect(trackColor(0)).toBe('#7fa58c');
    expect(trackColor(6)).toBe('#7fa58c');
  });
  it('mixes two hex colors', () => {
    expect(mixHex('#ffffff', '#000000', 0.5)).toBe('#808080');
    expect(mixHex('#7fa58c', '#000000', 1)).toBe('#7fa58c');
  });
});

describe('metaLine', () => {
  const media: EditorMedia = {
    session: 1,
    path: 'C:\\clips\\Replay.mp4',
    file_name: 'Replay.mp4',
    size_bytes: 412_000_000,
    duration_s: 90,
    bit_rate: null,
    modified_ms: 0,
    video: { width: 1920, height: 1080, fps: 60, codec: 'hevc' },
    tracks: [
      { index: 0, empty: false, file: null },
      { index: 1, empty: false, file: null },
      { index: 2, empty: true, file: null },
    ],
  };
  it('lists resolution, fps, codec, tracks, duration and size', () => {
    expect(metaLine(media)).toBe('1920×1080 · 60 fps · HEVC · 2 audio tracks · 1:30 · 412 MB');
  });
  it('counts only tracks that hold audio, as the timeline shows them', () => {
    const tracks = [media.tracks[0], media.tracks[2]];
    expect(metaLine({ ...media, tracks })).toBe('1920×1080 · 60 fps · HEVC · 1 audio track · 1:30 · 412 MB');
  });
  it('drops the video parts for an audio-only file and singularises one track', () => {
    expect(metaLine({ ...media, video: null, tracks: [media.tracks[0]] })).toBe('1 audio track · 1:30 · 412 MB');
  });
});

describe('fileName', () => {
  it('keeps the last part of a Windows or POSIX path', () => {
    expect(fileName('C:\\Users\\ledan\\Videos\\Replay 1.mp4')).toBe('Replay 1.mp4');
    expect(fileName('/tmp/a.mkv')).toBe('a.mkv');
    expect(fileName('a.mp4')).toBe('a.mp4');
  });
});
