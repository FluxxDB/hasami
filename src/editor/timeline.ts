// Pure timeline model for the Quick Trim editor. Every function returns a new Timeline (or the
// same object when nothing changed, so history can skip no-op pushes). Times are seconds.
//
// One video track whose segments are always contiguous from 0 (their `at` is derived), plus one
// audio track per source audio stream whose segments sit wherever they sit.

export interface Seg {
  id: string;
  link: string; // segments created by the same cut share a link
  src: [number, number]; // source range, seconds
  at: number; // timeline start, seconds
}

export interface AudioTrack {
  index: number; // source audio stream index
  segs: Seg[];
  gainDb: number;
  muted: boolean;
}

export interface Timeline {
  sourceDuration: number;
  fps: number;
  video: Seg[];
  audio: AudioTrack[];
  nextId: number;
}

export const MIN_GAIN_DB = -60; // shown as −∞
export const MAX_GAIN_DB = 6;

// Comparison slack for times that went through float arithmetic.
const EPS = 1e-9;

export function createTimeline(sourceDuration: number, fps: number, audioIndexes: number[]): Timeline {
  let n = 0;
  const mk = (): Seg => ({ id: `S${n++}`, link: 'L0', src: [0, sourceDuration], at: 0 });
  const video = [mk()];
  const audio = audioIndexes.map((index) => ({ index, segs: [mk()], gainDb: 0, muted: false }));
  return { sourceDuration, fps, video, audio, nextId: n };
}

export function quantize(t: number, fps: number): number {
  return Math.round(t * fps) / fps;
}

export function segLen(s: Seg): number {
  return s.src[1] - s.src[0];
}

const segEnd = (s: Seg): number => s.at + segLen(s);

export function videoEnd(tl: Timeline): number {
  const last = tl.video[tl.video.length - 1];
  return last ? segEnd(last) : 0;
}

export function cutCount(tl: Timeline): number {
  return tl.video.length - 1;
}

/** Re-derives each video seg's `at` as the sum of the lengths before it. */
function layoutVideo(video: Seg[]): Seg[] {
  let t = 0;
  return video.map((s) => {
    const placed = { ...s, at: t };
    t += segLen(s);
    return placed;
  });
}

const sortByAt = (segs: Seg[]): Seg[] => [...segs].sort((a, b) => a.at - b.at);

// --- cut -----------------------------------------------------------------------------------

interface IdCounter {
  n: number;
}

/** Splits every seg of `segs` that strictly contains `t`. `rightLink` picks the right piece's link. */
function splitSegs(segs: Seg[], t: number, ids: IdCounter, rightLink: (link: string) => string): Seg[] {
  const out: Seg[] = [];
  for (const s of segs) {
    if (s.at + EPS < t && t < segEnd(s) - EPS) {
      const mid = s.src[0] + (t - s.at);
      out.push({ ...s, src: [s.src[0], mid] });
      out.push({ id: `S${ids.n++}`, link: rightLink(s.link), src: [mid, s.src[1]], at: t });
    } else {
      out.push(s);
    }
  }
  return out;
}

export function cutAt(tl: Timeline, t: number): Timeline {
  const q = quantize(t, tl.fps);
  const ids: IdCounter = { n: tl.nextId };
  // Right pieces of segs that shared a link get one new shared link.
  const newLinks = new Map<string, string>();
  const rightLink = (link: string): string => {
    let l = newLinks.get(link);
    if (l === undefined) {
      l = `L${ids.n++}`;
      newLinks.set(link, l);
    }
    return l;
  };
  const video = splitSegs(tl.video, q, ids, rightLink);
  const audio = tl.audio.map((a) => ({ ...a, segs: splitSegs(a.segs, q, ids, rightLink) }));
  if (ids.n === tl.nextId) return tl; // nothing was split
  return { ...tl, video: layoutVideo(video), audio, nextId: ids.n };
}

// --- delete --------------------------------------------------------------------------------

/** Ids of every seg, on any track, that shares a link with `segId` (including itself). */
export function linkedIds(tl: Timeline, segId: string): string[] {
  const all = [...tl.video, ...tl.audio.flatMap((a) => a.segs)];
  const seg = all.find((s) => s.id === segId);
  if (!seg) return [];
  return all.filter((s) => s.link === seg.link).map((s) => s.id);
}

/**
 * Removes the material in [a, b) from an audio track and closes the gap. `rightLink` names the
 * link of a split's right-hand piece, so a track never ends up with two segs sharing a link.
 */
function rippleAudioRange(
  segs: Seg[],
  a: number,
  b: number,
  ids: IdCounter,
  rightLink: (link: string) => string,
): Seg[] {
  const cut = splitSegs(splitSegs(segs, b, ids, rightLink), a, ids, rightLink);
  const gap = b - a;
  const out: Seg[] = [];
  for (const s of cut) {
    if (s.at >= a - EPS && segEnd(s) <= b + EPS) continue; // inside the removed range
    out.push(s.at >= b - EPS ? { ...s, at: s.at - gap } : s);
  }
  return out;
}

export function deleteSegs(tl: Timeline, ids: string[], linked: boolean): Timeline {
  const wanted = new Set(ids);
  const removedVideo = tl.video.filter((s) => wanted.has(s.id));
  if (removedVideo.length === tl.video.length) return tl; // never delete the last video
  const touchesAudio = tl.audio.some((a) => a.segs.some((s) => wanted.has(s.id)));
  if (removedVideo.length === 0 && !touchesAudio) return tl;

  // Selected audio segs go first, in place (silence, no ripple). Doing this before the video
  // ripple keeps their ids valid even if the ripple would have split them.
  let audio = tl.audio.map((a) => ({ ...a, segs: a.segs.filter((s) => !wanted.has(s.id)) }));
  let nextId = tl.nextId;

  if (removedVideo.length > 0 && linked) {
    const counter: IdCounter = { n: nextId };
    // Right to left so earlier ranges keep their coordinates.
    for (const r of [...removedVideo].reverse()) {
      const a = r.at;
      const b = segEnd(r);
      // Right-hand remnants of one old link share one new link across tracks, as in cutAt.
      const fresh = new Map<string, string>();
      const rightLink = (link: string): string => {
        let l = fresh.get(link);
        if (l === undefined) {
          l = `L${counter.n++}`;
          fresh.set(link, l);
        }
        return l;
      };
      audio = audio.map((tr) => ({ ...tr, segs: rippleAudioRange(tr.segs, a, b, counter, rightLink) }));
    }
    nextId = counter.n;
  }

  const video = layoutVideo(tl.video.filter((s) => !wanted.has(s.id)));
  return { ...tl, video, audio, nextId };
}

// --- edge drag -----------------------------------------------------------------------------

const clamp = (x: number, lo: number, hi: number): number => Math.min(Math.max(x, lo), hi);

export function moveEdge(
  tl: Timeline,
  segId: string,
  edge: 'start' | 'end',
  toT: number,
  linked: boolean,
): Timeline {
  const q = quantize(toT, tl.fps);
  const frame = 1 / tl.fps;
  const D = tl.sourceDuration;

  const vi = tl.video.findIndex((s) => s.id === segId);
  if (vi >= 0) return moveVideoEdge(tl, vi, edge, q, linked, frame, D);

  for (const tr of tl.audio) {
    const si = tr.segs.findIndex((s) => s.id === segId);
    if (si >= 0) return moveAudioEdge(tl, tr.index, segId, edge, q, frame, D);
  }
  return tl;
}

function moveVideoEdge(
  tl: Timeline,
  vi: number,
  edge: 'start' | 'end',
  q: number,
  linked: boolean,
  frame: number,
  D: number,
): Timeline {
  const seg = tl.video[vi];
  const oldEnd = segEnd(seg);
  // `d` is the change in the moved src boundary (src[1] for end, src[0] for start).
  let d: number;
  let src: [number, number];
  if (edge === 'end') {
    const next = clamp(seg.src[1] + (q - oldEnd), seg.src[0] + frame, D);
    d = next - seg.src[1];
    src = [seg.src[0], next];
  } else {
    const next = clamp(seg.src[0] + (q - seg.at), 0, seg.src[1] - frame);
    d = next - seg.src[0];
    src = [next, seg.src[1]];
  }
  if (Math.abs(d) < EPS) return tl;
  const lenChange = edge === 'end' ? d : -d;

  const video = layoutVideo(tl.video.map((s, i) => (i === vi ? { ...s, src } : s)));
  if (!linked) return { ...tl, video };

  // Linked: audio segs with the same link whose matching edge sits on the dragged video edge get
  // the same src change. Anything else on an audio track is ordinary audio. Segs after the edge
  // shift by the length that track's moved seg actually changed, so a drag never overlaps them.
  const aligned = (s: Seg): boolean =>
    s.link === seg.link &&
    (edge === 'end' ? Math.abs(segEnd(s) - oldEnd) < EPS : Math.abs(s.at - seg.at) < EPS);
  const audio = tl.audio.map((tr) => {
    let trackShift = lenChange;
    const moved = tr.segs.map((s) => {
      if (!aligned(s)) return s;
      const next: [number, number] =
        edge === 'end'
          ? [s.src[0], clamp(s.src[1] + d, s.src[0] + frame, D)]
          : [clamp(s.src[0] + d, 0, s.src[1] - frame), s.src[1]];
      trackShift = edge === 'end' ? next[1] - s.src[1] : s.src[0] - next[0];
      return { ...s, src: next };
    });
    return {
      ...tr,
      segs: moved.map((s) =>
        !aligned(s) && s.at >= oldEnd - EPS ? { ...s, at: s.at + trackShift } : s,
      ),
    };
  });
  return { ...tl, video, audio };
}

function moveAudioEdge(
  tl: Timeline,
  trackIndex: number,
  segId: string,
  edge: 'start' | 'end',
  q: number,
  frame: number,
  D: number,
): Timeline {
  const track = tl.audio.find((a) => a.index === trackIndex)!;
  const seg = track.segs.find((s) => s.id === segId)!;
  const others = track.segs.filter((s) => s.id !== segId);
  const prevEnd = Math.max(0, ...others.filter((s) => s.at < seg.at).map(segEnd));
  const nextAt = Math.min(Infinity, ...others.filter((s) => s.at > seg.at).map((s) => s.at));

  let moved: Seg;
  if (edge === 'start') {
    // `at` and src[0] move together, so the audio under the cursor stays in sync.
    const lo = Math.max(prevEnd - seg.at, -seg.src[0]);
    const hi = segLen(seg) - frame;
    const delta = clamp(q - seg.at, lo, hi);
    moved = { ...seg, at: seg.at + delta, src: [seg.src[0] + delta, seg.src[1]] };
  } else {
    const hi = Math.min(D, seg.src[1] + (nextAt - segEnd(seg)));
    const next = clamp(seg.src[1] + (q - segEnd(seg)), seg.src[0] + frame, hi);
    moved = { ...seg, src: [seg.src[0], next] };
  }
  if (moved.at === seg.at && moved.src[0] === seg.src[0] && moved.src[1] === seg.src[1]) return tl;
  const audio = tl.audio.map((a) =>
    a.index === trackIndex ? { ...a, segs: a.segs.map((s) => (s.id === segId ? moved : s)) } : a,
  );
  return { ...tl, audio };
}

// --- gain / mute ---------------------------------------------------------------------------

export function setGain(tl: Timeline, index: number, gainDb: number): Timeline {
  const g = clamp(gainDb, MIN_GAIN_DB, MAX_GAIN_DB);
  if (tl.audio.find((a) => a.index === index)?.gainDb === g) return tl;
  return { ...tl, audio: tl.audio.map((a) => (a.index === index ? { ...a, gainDb: g } : a)) };
}

export function setMuted(tl: Timeline, index: number, muted: boolean): Timeline {
  if (tl.audio.find((a) => a.index === index)?.muted === muted) return tl;
  return { ...tl, audio: tl.audio.map((a) => (a.index === index ? { ...a, muted } : a)) };
}

// --- snapping ------------------------------------------------------------------------------

export function snapCandidates(
  tl: Timeline,
  playhead: number,
  exclude?: { segId: string; edge: 'start' | 'end' },
  linked = true,
): number[] {
  const dragged = exclude ? tl.video.find((s) => s.id === exclude.segId) : undefined;
  // Dragging a video edge moves everything from that edge on (the ripple). With `linked`, audio
  // from that point on ripples too, and the linked audio edges on the same side move with it.
  // None of those can be snap targets, or the edge snaps back to where it started. Without
  // `linked`, audio does not move and stays a target.
  const from = dragged && exclude ? (exclude.edge === 'start' ? dragged.at : segEnd(dragged)) : Infinity;
  const movesWithDrag = (s: Seg, isVideo: boolean, edge: 'start' | 'end'): boolean => {
    if (!dragged || !exclude) return false;
    if (isVideo) return (edge === 'start' ? s.at : segEnd(s)) >= from - EPS;
    if (!linked) return false;
    const t = edge === 'start' ? s.at : segEnd(s);
    return t >= from - EPS || (s.link === dragged.link && edge === exclude.edge);
  };

  const out = [playhead];
  if (!dragged) out.push(videoEnd(tl));
  const add = (s: Seg, isVideo: boolean) => {
    const lit = (edge: 'start' | 'end') => exclude?.segId === s.id && exclude.edge === edge;
    if (!lit('start') && !movesWithDrag(s, isVideo, 'start')) out.push(s.at);
    if (!lit('end') && !movesWithDrag(s, isVideo, 'end')) out.push(segEnd(s));
  };
  tl.video.forEach((s) => add(s, true));
  tl.audio.forEach((a) => a.segs.forEach((s) => add(s, false)));
  return out;
}

/** The nearest candidate within `thresholdS` of `t`, otherwise `t` itself. */
export function snap(t: number, candidates: number[], thresholdS: number): number {
  let best = t;
  let bestDist = thresholdS;
  for (const c of candidates) {
    const d = Math.abs(c - t);
    if (d <= bestDist) {
      best = c;
      bestDist = d;
    }
  }
  return best;
}

// --- export --------------------------------------------------------------------------------

export interface ExportRequest {
  session: number;
  input: string;
  mode: 'quick' | 'save_raw' | 'audio';
  fit_20mb: boolean;
  duration_s: number;
  video: { src_start: number; src_end: number }[];
  audio: { index: number; gain_db: number; pieces: { at: number; src_start: number; src_end: number }[] }[];
}

export function toExportRequest(
  tl: Timeline,
  session: number,
  input: string,
  mode: ExportRequest['mode'],
  fit20: boolean,
): ExportRequest {
  const end = videoEnd(tl);
  const audio: ExportRequest['audio'] = [];
  for (const tr of tl.audio) {
    if (tr.muted) continue;
    const pieces = sortByAt(tr.segs).flatMap((s) => {
      const len = Math.min(segLen(s), end - s.at); // clip to the end of the video
      return len > EPS ? [{ at: s.at, src_start: s.src[0], src_end: s.src[0] + len }] : [];
    });
    if (pieces.length > 0) audio.push({ index: tr.index, gain_db: tr.gainDb, pieces });
  }
  return {
    session,
    input,
    mode,
    fit_20mb: fit20,
    duration_s: end,
    video: mode === 'audio' ? [] : tl.video.map((s) => ({ src_start: s.src[0], src_end: s.src[1] })),
    audio,
  };
}

/** True when some unmuted track has material before the end of the video. */
export function hasAudibleAudio(tl: Timeline): boolean {
  const end = videoEnd(tl);
  return tl.audio.some((a) => !a.muted && a.segs.some((s) => s.at < end - EPS && segLen(s) > EPS));
}
