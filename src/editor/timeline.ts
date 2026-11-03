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
  masterDb: number; // one gain on the final mix, exported in every mode
}

export const MIN_GAIN_DB = -60; // shown as −∞
export const MAX_GAIN_DB = 6;
export const MASTER_MAX_DB = 12; // no limiter: what is set is what is exported
export const MASTER_STEP_DB = 3; // the master − / + buttons

/**
 * A track nobody hears: muted, or its fader at the bottom (−∞). The preview plays it at gain 0,
 * and exports leave it out entirely, so the two always agree.
 */
export function isSilent(gainDb: number, muted: boolean): boolean {
  return muted || gainDb <= MIN_GAIN_DB;
}

/** The master fader at the bottom (−∞): the whole mix is silent. */
const masterOff = (tl: Timeline): boolean => tl.masterDb <= MIN_GAIN_DB;

// Comparison slack for times that went through float arithmetic.
const EPS = 1e-9;

export function createTimeline(sourceDuration: number, fps: number, audioIndexes: number[]): Timeline {
  let n = 0;
  const mk = (): Seg => ({ id: `S${n++}`, link: 'L0', src: [0, sourceDuration], at: 0 });
  const video = [mk()];
  const audio = audioIndexes.map((index) => ({ index, segs: [mk()], gainDb: 0, muted: false }));
  return { sourceDuration, fps, video, audio, nextId: n, masterDb: 0 };
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

/**
 * Splits every seg of `segs` that strictly contains `t` (with `only`, just the listed ids).
 * `rightLink` picks the right piece's link.
 */
function splitSegs(
  segs: Seg[],
  t: number,
  ids: IdCounter,
  rightLink: (link: string) => string,
  only?: ReadonlySet<string>,
): Seg[] {
  const out: Seg[] = [];
  for (const s of segs) {
    if (s.at + EPS < t && t < segEnd(s) - EPS && (!only || only.has(s.id))) {
      const mid = s.src[0] + (t - s.at);
      out.push({ ...s, src: [s.src[0], mid] });
      out.push({ id: `S${ids.n++}`, link: rightLink(s.link), src: [mid, s.src[1]], at: t });
    } else {
      out.push(s);
    }
  }
  return out;
}

/** Splits every track at `t`, or with `only`, just the segs whose id is listed. */
export function cutAt(tl: Timeline, t: number, only?: string[]): Timeline {
  const wanted = only && new Set(only);
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
  let video = splitSegs(tl.video, q, ids, rightLink, wanted);
  let audio = tl.audio.map((a) => ({ ...a, segs: splitSegs(a.segs, q, ids, rightLink, wanted) }));
  if (ids.n === tl.nextId) return tl; // nothing was split
  if (wanted) {
    // A link group split only in part: the segs left whole across `t` no longer pair with either
    // piece, so each gets a link of its own (else linking them back would select, and Delete
    // remove, material far longer than the piece clicked).
    const spans = (x: Seg): boolean => x.at + EPS < q && q < segEnd(x) - EPS;
    const all = [...tl.video, ...tl.audio.flatMap((a) => a.segs)];
    const cutLinks = new Set(all.filter((x) => spans(x) && wanted.has(x.id)).map((x) => x.link));
    const detached = new Map<string, string>();
    for (const x of all) {
      if (spans(x) && !wanted.has(x.id) && cutLinks.has(x.link)) detached.set(x.id, `L${ids.n++}`);
    }
    if (detached.size > 0) {
      const relink = (segs: Seg[]): Seg[] =>
        segs.map((x) => (detached.has(x.id) ? { ...x, link: detached.get(x.id)! } : x));
      video = relink(video);
      audio = audio.map((a) => ({ ...a, segs: relink(a.segs) }));
    }
  }
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

/**
 * Where the playhead goes after `deleteSegs(tl, ids, linked)`: the timeline time of the leftmost
 * removed video range's start, which after the ripple is where its neighbours meet (the end of
 * what is left when the tail went, 0 when the head did). Null when no video is removed, so an
 * audio-only delete, or one `deleteSegs` refuses, leaves the playhead alone. Video ripples the
 * same way whether or not Linked is on; `linked` is in the signature to mirror `deleteSegs`.
 */
export function deleteJoin(tl: Timeline, ids: string[], _linked: boolean): number | null {
  const wanted = new Set(ids);
  const removed = tl.video.filter((s) => wanted.has(s.id));
  if (removed.length === 0 || removed.length === tl.video.length) return null;
  return Math.min(...removed.map((s) => s.at));
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

  // Linked: an audio seg with the same link whose matching edge sits on the dragged video edge
  // gets the same src change, and the segs after it on its track shift by the length it actually
  // changed. A track with no such seg is ordinary audio: its segs after the edge shift by the
  // video's length change, but never into a seg that stays put. Either way a drag cannot overlap.
  const aligned = (s: Seg): boolean =>
    s.link === seg.link &&
    (edge === 'end' ? Math.abs(segEnd(s) - oldEnd) < EPS : Math.abs(s.at - seg.at) < EPS);
  const audio = tl.audio.map((tr) => {
    const hit = tr.segs.find(aligned);
    let from = oldEnd;
    let shift = lenChange;
    let hitSrc: [number, number] | undefined;
    if (hit) {
      hitSrc =
        edge === 'end'
          ? [hit.src[0], clamp(hit.src[1] + d, hit.src[0] + frame, D)]
          : [clamp(hit.src[0] + d, 0, hit.src[1] - frame), hit.src[1]];
      from = segEnd(hit);
      shift = edge === 'end' ? hitSrc[1] - hit.src[1] : hit.src[0] - hitSrc[0];
    } else if (shift < 0) {
      // Shrinking: followers may close up only as far as the segs that do not move.
      const followers = tr.segs.filter((s) => s.at >= from - EPS);
      const fixedEnd = Math.max(0, ...tr.segs.filter((s) => s.at < from - EPS).map(segEnd));
      if (followers.length > 0) {
        const room = Math.min(...followers.map((s) => s.at)) - fixedEnd;
        shift = Math.max(shift, -Math.max(room, 0));
      }
    }
    return {
      ...tr,
      segs: tr.segs.map((s) => {
        if (s === hit && hitSrc) return { ...s, src: hitSrc };
        return s.at >= from - EPS ? { ...s, at: s.at + shift } : s;
      }),
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
  const track = tl.audio.find((a) => a.index === index);
  if (!track || track.gainDb === g) return tl;
  return { ...tl, audio: tl.audio.map((a) => (a.index === index ? { ...a, gainDb: g } : a)) };
}

export function setMuted(tl: Timeline, index: number, muted: boolean): Timeline {
  const track = tl.audio.find((a) => a.index === index);
  if (!track || track.muted === muted) return tl;
  return { ...tl, audio: tl.audio.map((a) => (a.index === index ? { ...a, muted } : a)) };
}

// --- master volume -------------------------------------------------------------------------

/** The gain on the final mix, clamped to [−∞ (MIN_GAIN_DB), +12 dB]. */
export function setMaster(tl: Timeline, db: number): Timeline {
  const g = clamp(db, MIN_GAIN_DB, MASTER_MAX_DB);
  return tl.masterDb === g ? tl : { ...tl, masterDb: g };
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

/** A rectangle of the source picture, in its pixels: the black border found on open is what lies outside it. */
export interface Crop {
  x: number;
  y: number;
  w: number;
  h: number;
}

export interface ExportRequest {
  session: number;
  input: string;
  mode: 'quick' | 'save_raw' | 'audio';
  fit_20mb: boolean;
  duration_s: number;
  video: { src_start: number; src_end: number }[];
  audio: { index: number; gain_db: number; pieces: { at: number; src_start: number; src_end: number }[] }[];
  master_db: number; // never -60 or lower: at −∞ `audio` is empty and this is 0
  crop: Crop | null; // the picture to keep (video exports scale it back up to the source size); null keeps it all
}

export function toExportRequest(
  tl: Timeline,
  session: number,
  input: string,
  mode: ExportRequest['mode'],
  fit20: boolean,
  crop: Crop | null = null,
): ExportRequest {
  const end = videoEnd(tl);
  const audio: ExportRequest['audio'] = [];
  // A master at −∞ is silence on the whole mix, so no track is sent (as for a muted one).
  for (const tr of masterOff(tl) ? [] : tl.audio) {
    if (isSilent(tr.gainDb, tr.muted)) continue;
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
    // At −∞ no audio is sent, so the gain is moot; 0 keeps -60 or lower from reaching Rust.
    master_db: masterOff(tl) ? 0 : tl.masterDb,
    // Export audio has no picture to crop.
    crop: mode === 'audio' ? null : crop,
  };
}

/** True when some track that isn't silent (muted or at −∞, or the master at −∞) has material before the end of the video. */
export function hasAudibleAudio(tl: Timeline): boolean {
  if (masterOff(tl)) return false;
  const end = videoEnd(tl);
  const live = (s: Seg) => s.at < end - EPS && segLen(s) > EPS;
  return tl.audio.some((a) => !isSilent(a.gainDb, a.muted) && a.segs.some(live));
}
