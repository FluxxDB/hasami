import { describe, expect, it } from 'vitest';
import {
  EDGE_HIT_PX,
  SNAP_PX,
  edgeAt,
  faderPos,
  fitZoom,
  rulerStep,
  timeToX,
  xToTime,
  zoomAround,
  zoomPos,
} from './geometry';

describe('time and pixels', () => {
  it('maps time to x and back', () => {
    expect(timeToX(10, 20, 50)).toBe(150);
    expect(xToTime(150, 20, 50)).toBe(10);
  });
  it('exports the hit and snap distances', () => {
    expect(SNAP_PX).toBe(8);
    expect(EDGE_HIT_PX).toBe(6);
  });
});

describe('edgeAt', () => {
  // At 20 px/s with no scroll: A spans x 0..200, B spans x 300..400.
  const segs = [
    { id: 'A', at: 0, len: 10 },
    { id: 'B', at: 15, len: 5 },
  ];
  it('finds the end of A within 6px either side', () => {
    expect(edgeAt(segs, 205, 20, 0)).toEqual({ segId: 'A', edge: 'end' });
    expect(edgeAt(segs, 195, 20, 0)).toEqual({ segId: 'A', edge: 'end' });
  });
  it('misses beyond 6px', () => {
    expect(edgeAt(segs, 207, 20, 0)).toBeNull();
    expect(edgeAt(segs, 193, 20, 0)).toBeNull();
  });
  it('finds start edges and respects scroll', () => {
    expect(edgeAt(segs, 302, 20, 0)).toEqual({ segId: 'B', edge: 'start' });
    expect(edgeAt(segs, 252, 20, 50)).toEqual({ segId: 'B', edge: 'start' });
  });
  it('prefers the segment the pointer is inside when two edges touch', () => {
    const touching = [
      { id: 'A', at: 0, len: 10 },
      { id: 'B', at: 10, len: 5 },
    ];
    expect(edgeAt(touching, 198, 20, 0)).toEqual({ segId: 'A', edge: 'end' });
    expect(edgeAt(touching, 202, 20, 0)).toEqual({ segId: 'B', edge: 'start' });
  });
});

describe('zoom', () => {
  it('fits a duration into a width', () => {
    expect(fitZoom(90, 1080)).toBe(12);
  });
  it('clamps the fit to 2..2000 px/s', () => {
    expect(fitZoom(10_000, 1000)).toBe(2);
    expect(fitZoom(0.1, 1000)).toBe(2000);
    expect(fitZoom(0, 1000)).toBe(2000);
  });
  it('keeps the time under the anchor fixed when zooming in and out', () => {
    for (const [pps, factor, anchor, scroll] of [
      [20, 1.5, 300, 100],
      [20, 0.5, 300, 900],
      [12, 4, 640, 0],
    ]) {
      const before = xToTime(anchor, pps, scroll);
      const z = zoomAround(pps, factor, anchor, scroll);
      expect(z.pxPerSecond).toBeCloseTo(pps * factor, 9);
      expect(xToTime(anchor, z.pxPerSecond, z.scrollX)).toBeCloseTo(before, 9);
    }
  });
  it('clamps zoom to 2..2000 px/s', () => {
    expect(zoomAround(1500, 2, 0, 0).pxPerSecond).toBe(2000);
    expect(zoomAround(3, 0.5, 0, 0).pxPerSecond).toBe(2);
  });
  it('never scrolls before time 0', () => {
    expect(zoomAround(20, 0.5, 300, 10).scrollX).toBe(0);
  });
  it('places the zoom slider on a log scale', () => {
    expect(zoomPos(2)).toBe(0);
    expect(zoomPos(2000)).toBe(1);
    expect(zoomPos(Math.sqrt(2 * 2000))).toBeCloseTo(0.5, 9);
  });
});

describe('rulerStep', () => {
  it('picks the smallest nice interval at least 80px apart', () => {
    expect(rulerStep(20)).toEqual({ major: 5, minor: 1 });
    expect(rulerStep(12)).toEqual({ major: 10, minor: 2 });
    expect(rulerStep(100)).toEqual({ major: 1, minor: 0.2 });
    expect(rulerStep(2)).toEqual({ major: 60, minor: 10 });
  });
});

describe('faderPos', () => {
  it('puts 0 dB about 71% along and the ends at 0 and 1', () => {
    expect(faderPos(0)).toBeCloseTo(0.708, 3);
    expect(faderPos(6)).toBe(1);
    expect(faderPos(-60)).toBe(0);
    expect(faderPos(-6)).toBeLessThan(faderPos(0));
  });
});
