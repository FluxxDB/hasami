// Pixel maths for the editor timeline. `x` is measured from the left edge of the lanes; `scrollX`
// is how many pixels of timeline are scrolled off to the left. Pure, so it's unit-tested.

export const SNAP_PX = 8;
export const EDGE_HIT_PX = 6;

export const MIN_PX_PER_SECOND = 2;
export const MAX_PX_PER_SECOND = 2000;

const clamp = (x: number, lo: number, hi: number): number => Math.min(Math.max(x, lo), hi);

export function timeToX(t: number, pxPerSecond: number, scrollX: number): number {
  return t * pxPerSecond - scrollX;
}

export function xToTime(x: number, pxPerSecond: number, scrollX: number): number {
  return (x + scrollX) / pxPerSecond;
}

/**
 * The segment edge within EDGE_HIT_PX of `x`, nearest first. When two edges are equally near
 * (touching segments), the one belonging to the segment under the pointer wins.
 */
export function edgeAt(
  segs: { id: string; at: number; len: number }[],
  x: number,
  pxPerSecond: number,
  scrollX: number,
): { segId: string; edge: 'start' | 'end' } | null {
  let best: { segId: string; edge: 'start' | 'end' } | null = null;
  let bestScore = Infinity;
  for (const s of segs) {
    const x0 = timeToX(s.at, pxPerSecond, scrollX);
    const x1 = timeToX(s.at + s.len, pxPerSecond, scrollX);
    for (const [edge, ex] of [
      ['start', x0],
      ['end', x1],
    ] as const) {
      const d = Math.abs(x - ex);
      if (d > EDGE_HIT_PX) continue;
      const inside = x >= x0 && x <= x1;
      const score = d + (inside ? 0 : 0.5); // half a pixel of preference for the inside edge
      if (score < bestScore) {
        bestScore = score;
        best = { segId: s.id, edge };
      }
    }
  }
  return best;
}

/** The zoom that shows `duration` seconds in `widthPx`, clamped to the zoom range. */
export function fitZoom(duration: number, widthPx: number): number {
  if (!(duration > 0)) return MAX_PX_PER_SECOND;
  return clamp(widthPx / duration, MIN_PX_PER_SECOND, MAX_PX_PER_SECOND);
}

/** Zooms by `factor` keeping the time under `anchorX` where it is (except it never scrolls past 0). */
export function zoomAround(
  pxPerSecond: number,
  factor: number,
  anchorX: number,
  scrollX: number,
): { pxPerSecond: number; scrollX: number } {
  const t = xToTime(anchorX, pxPerSecond, scrollX);
  const next = clamp(pxPerSecond * factor, MIN_PX_PER_SECOND, MAX_PX_PER_SECOND);
  return { pxPerSecond: next, scrollX: Math.max(0, t * next - anchorX) };
}

/** Where the zoom slider's knob sits for a zoom level: 0..1 on a log scale. */
export function zoomPos(pxPerSecond: number): number {
  const p = Math.log(pxPerSecond / MIN_PX_PER_SECOND) / Math.log(MAX_PX_PER_SECOND / MIN_PX_PER_SECOND);
  return clamp(p, 0, 1);
}

// Major ruler intervals (seconds) and the minor tick spacing under each.
const RULER_STEPS: { major: number; minor: number }[] = [
  { major: 1, minor: 0.2 },
  { major: 2, minor: 0.5 },
  { major: 5, minor: 1 },
  { major: 10, minor: 2 },
  { major: 15, minor: 5 },
  { major: 30, minor: 5 },
  { major: 60, minor: 10 },
  { major: 120, minor: 30 },
  { major: 300, minor: 60 },
  { major: 600, minor: 120 },
  { major: 900, minor: 300 },
  { major: 1800, minor: 300 },
  { major: 3600, minor: 600 },
];
const MIN_MAJOR_PX = 80;

/** The smallest "nice" labelled interval that leaves room for a label. */
export function rulerStep(pxPerSecond: number): { major: number; minor: number } {
  return RULER_STEPS.find((s) => s.major * pxPerSecond >= MIN_MAJOR_PX) ?? RULER_STEPS[RULER_STEPS.length - 1];
}

/**
 * Where a volume slider's knob sits for a gain: 0..1, with the square root of the linear
 * amplitude (as a fraction of +6 dB) as the taper, so 0 dB lands about 71% along.
 */
export function faderPos(gainDb: number, minDb = -60, maxDb = 6): number {
  if (gainDb <= minDb) return 0;
  return clamp(Math.sqrt(10 ** ((gainDb - maxDb) / 20)), 0, 1);
}
