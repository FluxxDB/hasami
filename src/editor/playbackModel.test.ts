import { describe, expect, it } from 'vitest';
import {
  audioSourceAt,
  cutPoints,
  formatTimecode,
  nextCut,
  prevCut,
  shuttle,
  stepFrames,
  STOPPED,
  videoSourceAt,
  type Shuttle,
} from './playbackModel';
import { createTimeline, cutAt, deleteSegs, linkedIds } from './timeline';

const tl0 = createTimeline(90, 60, [0, 1]);
const tl2 = cutAt(cutAt(tl0, 12), 31);
const deleted = deleteSegs(tl2, linkedIds(tl2, tl2.video[1].id), true);

describe('shuttle', () => {
  it('L taps from stopped go 1x, 2x, 4x, 8x and stay at 8x', () => {
    const seq: Shuttle[] = [];
    let s: Shuttle = STOPPED;
    for (let i = 0; i < 5; i++) {
      s = shuttle(s, 'L');
      seq.push(s);
    }
    expect(seq).toEqual([
      { dir: 1, speed: 1 },
      { dir: 1, speed: 2 },
      { dir: 1, speed: 4 },
      { dir: 1, speed: 8 },
      { dir: 1, speed: 8 },
    ]);
  });

  it('J mirrors L', () => {
    let s: Shuttle = STOPPED;
    for (let i = 0; i < 5; i++) s = shuttle(s, 'J');
    expect(s).toEqual({ dir: -1, speed: 8 });
    expect(shuttle(STOPPED, 'J')).toEqual({ dir: -1, speed: 1 });
  });

  it('J after 4x forward reverses at 1x, and L after reverse goes forward at 1x', () => {
    expect(shuttle({ dir: 1, speed: 4 }, 'J')).toEqual({ dir: -1, speed: 1 });
    expect(shuttle({ dir: -1, speed: 4 }, 'L')).toEqual({ dir: 1, speed: 1 });
  });

  it('K stops', () => {
    expect(shuttle({ dir: 1, speed: 8 }, 'K')).toEqual(STOPPED);
    expect(STOPPED).toEqual({ dir: 0, speed: 1 });
  });
});

describe('formatTimecode', () => {
  it('formats HH:MM:SS:FF', () => {
    expect(formatTimecode(21.7166, 60)).toBe('00:00:21:43');
    expect(formatTimecode(3725.5, 30)).toBe('01:02:05:15');
    expect(formatTimecode(0, 60)).toBe('00:00:00:00');
  });
  it('rolls a frame count that rounds up to a whole second into the seconds', () => {
    expect(formatTimecode(0.99999, 60)).toBe('00:00:01:00');
    expect(formatTimecode(12 - 1e-12, 60)).toBe('00:00:12:00');
  });
  it('never shows FF above the frame rate for fractional rates', () => {
    expect(formatTimecode(1 / 29.97, 29.97)).toBe('00:00:00:01');
    expect(formatTimecode(29 / 29.97, 29.97)).toBe('00:00:00:29');
    expect(formatTimecode(30 / 29.97, 29.97)).toBe('00:00:01:00');
  });
  it('clamps negative time to zero', () => {
    expect(formatTimecode(-1, 60)).toBe('00:00:00:00');
  });
});

describe('cut navigation', () => {
  it('cutPoints are 0, every boundary and the end, sorted and unique', () => {
    expect(cutPoints(tl2)).toEqual([0, 12, 31, 90]);
    expect(cutPoints(tl0)).toEqual([0, 90]);
    expect(cutPoints(deleted)).toEqual([0, 12, 71]);
  });
  it('prevCut / nextCut', () => {
    expect(prevCut(tl2, 12)).toBe(0);
    expect(prevCut(tl2, 12.5)).toBe(12);
    expect(prevCut(tl2, 0)).toBe(0);
    expect(nextCut(tl2, 12)).toBe(31);
    expect(nextCut(tl2, 0)).toBe(12);
    expect(nextCut(tl2, 95)).toBe(90);
    expect(nextCut(tl2, 90)).toBe(90);
  });
  it('ignores sub-microsecond distance from a cut', () => {
    expect(prevCut(tl2, 12 + 1e-9)).toBe(0);
    expect(nextCut(tl2, 12 - 1e-9)).toBe(31);
  });
});

describe('stepFrames', () => {
  it('steps by whole frames and clamps to [0, end]', () => {
    expect(stepFrames(1, 60, -1, 90)).toBe(59 / 60);
    expect(stepFrames(0, 60, -1, 90)).toBe(0);
    expect(stepFrames(89.99, 60, 60, 90)).toBe(90);
    expect(stepFrames(10, 60, 60, 90)).toBe(11);
  });
  it('snaps an off-grid playhead to the frame grid', () => {
    expect(stepFrames(1.004, 60, 1, 90)).toBeCloseTo(61 / 60, 9);
  });
});

describe('source mapping', () => {
  it('videoSourceAt maps timeline time through the ripple', () => {
    const hit = videoSourceAt(deleted, 12.5)!;
    expect(hit.seg.src).toEqual([31, 90]);
    expect(hit.srcT).toBe(31.5);
    expect(videoSourceAt(deleted, 5)!.srcT).toBe(5);
  });
  it('videoSourceAt: a boundary belongs to the segment that starts there', () => {
    expect(videoSourceAt(deleted, 12)!.srcT).toBe(31);
  });
  it('videoSourceAt is null outside [0, videoEnd)', () => {
    expect(videoSourceAt(deleted, -1)).toBeNull();
    expect(videoSourceAt(deleted, 71)).toBeNull();
    expect(videoSourceAt(deleted, 100)).toBeNull();
  });
  it('audioSourceAt finds the seg or reports silence', () => {
    const gap = deleteSegs(tl2, [tl2.audio[0].segs[1].id], true);
    const segs = gap.audio[0].segs;
    expect(audioSourceAt(segs, 5)!.srcT).toBe(5);
    expect(audioSourceAt(segs, 20)).toBeNull(); // the silence left by the delete
    expect(audioSourceAt(segs, 40)!.srcT).toBe(40);
    expect(audioSourceAt(segs, 12)).toBeNull(); // end of a seg is exclusive
    expect(audioSourceAt(segs, 31)!.srcT).toBe(31);
  });
});
