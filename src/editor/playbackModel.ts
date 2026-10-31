// Playback-side maths for the Quick Trim editor: J/K/L shuttle, timecode, cut navigation, and
// mapping timeline time to source time. Pure; the editor component owns the actual clock.

import { quantize, segLen, videoEnd, type Seg, type Timeline } from './timeline';

export interface Shuttle {
  dir: -1 | 0 | 1;
  speed: 1 | 2 | 4 | 8;
}

export const STOPPED: Shuttle = { dir: 0, speed: 1 };

const FASTER = { 1: 2, 2: 4, 4: 8, 8: 8 } as const;

/** DaVinci-style: repeated taps in one direction speed up; the other direction restarts at 1x. */
export function shuttle(s: Shuttle, key: 'J' | 'K' | 'L'): Shuttle {
  if (key === 'K') return STOPPED;
  const dir = key === 'L' ? 1 : -1;
  if (s.dir === dir) return { dir, speed: FASTER[s.speed] };
  return { dir, speed: 1 };
}

const pad2 = (n: number): string => String(n).padStart(2, '0');

/** "HH:MM:SS:FF" (non-drop). The playhead is shown on the nearest frame. */
export function formatTimecode(t: number, fps: number): string {
  const frames = Math.round(Math.max(0, t) * fps);
  // Non-drop: a "second" holds round(fps) frames, so 29.97 never repeats or skips a number.
  const nominal = Math.max(1, Math.round(fps));
  const secs = Math.floor(frames / nominal);
  const ff = frames % nominal;
  const h = Math.floor(secs / 3600);
  const m = Math.floor((secs % 3600) / 60);
  return `${pad2(h)}:${pad2(m)}:${pad2(secs % 60)}:${pad2(ff)}`;
}

const CUT_EPS = 1e-6;

/** 0, every video segment boundary, and the end of the video. Sorted, no duplicates. */
export function cutPoints(tl: Timeline): number[] {
  const points = [0, ...tl.video.map((s) => s.at + segLen(s)), videoEnd(tl)].sort((a, b) => a - b);
  return points.filter((p, i) => i === 0 || p - points[i - 1] > CUT_EPS);
}

/** The greatest cut point before `t`, or 0. */
export function prevCut(tl: Timeline, t: number): number {
  const before = cutPoints(tl).filter((c) => c < t - CUT_EPS);
  return before.length > 0 ? before[before.length - 1] : 0;
}

/** The least cut point after `t`, or the end of the video. */
export function nextCut(tl: Timeline, t: number): number {
  return cutPoints(tl).find((c) => c > t + CUT_EPS) ?? videoEnd(tl);
}

/** Moves `n` frames (negative = back) from `t`, landing on the frame grid inside [0, end]. */
export function stepFrames(t: number, fps: number, n: number, end: number): number {
  const stepped = Math.round(t * fps + n) / fps;
  return Math.min(Math.max(stepped, 0), end);
}

const SRC_EPS = 1e-9;

/** The seg covering timeline time `t` (start inclusive, end exclusive) and the source time in it. */
function sourceAt(segs: Seg[], t: number): { seg: Seg; srcT: number } | null {
  for (const seg of segs) {
    if (t >= seg.at - SRC_EPS && t < seg.at + segLen(seg) - SRC_EPS) {
      return { seg, srcT: seg.src[0] + (t - seg.at) };
    }
  }
  return null;
}

/** Which video segment is on screen at `t`; null outside [0, videoEnd). */
export function videoSourceAt(tl: Timeline, t: number): { seg: Seg; srcT: number } | null {
  return sourceAt(tl.video, t);
}

/** Which audio segment sounds at `t`; null in the silence between segments. */
export function audioSourceAt(segs: Seg[], t: number): { seg: Seg; srcT: number } | null {
  return sourceAt(segs, t);
}
