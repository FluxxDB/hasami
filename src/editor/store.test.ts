import { describe, expect, it } from 'vitest';
import type { EditorMedia } from './api';
import { EditorStore } from './store.svelte';
import { cutAt, deleteSegs, videoEnd } from './timeline';

const media = (over: Partial<EditorMedia> = {}): EditorMedia => ({
  session: 7,
  path: 'C:\\clips\\Replay.mp4',
  file_name: 'Replay.mp4',
  size_bytes: 412e6,
  duration_s: 90,
  bit_rate: null,
  video: { width: 1920, height: 1080, fps: 60, codec: 'hevc' },
  tracks: [
    { index: 0, empty: false, file: 'a0.m4a' },
    { index: 1, empty: true, file: null },
    { index: 2, empty: false, file: 'a2.m4a' },
  ],
  ...over,
});

describe('EditorStore', () => {
  it('load builds a timeline with one audio track per non-empty source track', () => {
    const s = new EditorStore();
    s.load(media());
    expect(s.timeline.audio.map((a) => a.index)).toEqual([0, 2]);
    expect(s.timeline.sourceDuration).toBe(90);
    expect(s.timeline.fps).toBe(60);
    expect(videoEnd(s.timeline)).toBe(90);
  });

  it('is not edited after load and is after one edit', () => {
    const s = new EditorStore();
    s.load(media());
    expect(s.edited).toBe(false);
    s.edit(cutAt(s.timeline, 12));
    expect(s.edited).toBe(true);
    expect(s.timeline.video).toHaveLength(2);
  });

  it('load resets edits, selection, playhead and per-clip data', () => {
    const s = new EditorStore();
    s.load(media());
    s.edit(cutAt(s.timeline, 12));
    s.selection = ['S0'];
    s.playhead = 30;
    s.peaks = { 0: { rate: 10, values: [0.5] } };
    s.thumbs = [{ t: 0, file: 'x.jpg' }];
    s.load(media({ session: 8 }));
    expect(s.edited).toBe(false);
    expect(s.selection).toEqual([]);
    expect(s.playhead).toBe(0);
    expect(s.peaks).toEqual({});
    expect(s.thumbs).toEqual([]);
  });

  it('estimates the quick export size from the source rate and the kept video', () => {
    const s = new EditorStore();
    s.load(media());
    // Keep 0..44 s: cut at 44 and delete the tail.
    const cut = cutAt(s.timeline, 44);
    s.edit(deleteSegs(cut, [cut.video[1].id], true));
    expect(videoEnd(s.timeline)).toBeCloseTo(44, 9);
    // 412e6 / 90 × 44 = 201.42e6 ("≈ 201.4 MB").
    expect(Math.abs(s.estimateBytes - (412e6 / 90) * 44)).toBeLessThan(1e3);
    expect(s.estimateBytes).toBeCloseTo(201.4e6, -5);
  });

  it('estimates nothing before a clip is loaded', () => {
    expect(new EditorStore().estimateBytes).toBe(0);
  });
});
