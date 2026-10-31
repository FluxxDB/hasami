import { describe, expect, it } from 'vitest';
import { DRIFT_TOLERANCE_S, trackGain, videoTarget } from './engine';
import { createTimeline, cutAt, deleteSegs } from './timeline';

describe('trackGain', () => {
  it('is 10^(dB/20), and 0 when muted or at the bottom of the fader', () => {
    expect(trackGain(0, false)).toBe(1);
    expect(trackGain(-6, false)).toBeCloseTo(0.501, 3);
    expect(trackGain(6, false)).toBeCloseTo(1.995, 3);
    expect(trackGain(-60, false)).toBe(0);
    expect(trackGain(-80, false)).toBe(0);
    expect(trackGain(0, true)).toBe(0);
  });
  it('keeps the drift tolerance at 40 ms', () => {
    expect(DRIFT_TOLERANCE_S).toBe(0.04);
  });
});

describe('videoTarget', () => {
  // 0–12 then 31–90 of the source (12–31 deleted), at 60 fps.
  const cut = cutAt(cutAt(createTimeline(90, 60, [0]), 12), 31);
  const tl = deleteSegs(cut, [cut.video[1].id], true);

  it('maps timeline time to the segment and source time on screen', () => {
    expect(videoTarget(tl, 5)).toEqual({ index: 0, srcT: 5 });
    expect(videoTarget(tl, 12)).toEqual({ index: 1, srcT: 31 });
    expect(videoTarget(tl, 20)).toEqual({ index: 1, srcT: 39 });
  });
  it('shows the last frame for a playhead parked at the end', () => {
    const t = videoTarget(tl, 71);
    expect(t?.index).toBe(1);
    expect(t?.srcT).toBeCloseTo(90 - 1 / 60, 9);
  });
  it('clamps before the start', () => {
    expect(videoTarget(tl, -1)).toEqual({ index: 0, srcT: 0 });
  });
});
