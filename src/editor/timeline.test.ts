import { describe, expect, it } from 'vitest';
import {
  createTimeline,
  cutAt,
  cutCount,
  deleteSegs,
  hasAudibleAudio,
  linkedIds,
  MIN_GAIN_DB,
  moveEdge,
  quantize,
  segLen,
  setGain,
  setMuted,
  snap,
  snapCandidates,
  toExportRequest,
  videoEnd,
  type Seg,
  type Timeline,
} from './timeline';

const D = 90;
const FPS = 60;
const tl0 = createTimeline(D, FPS, [0, 1]);
const tl2 = cutAt(cutAt(tl0, 12), 31);

const src = (segs: Seg[]) => segs.map((s) => s.src);
const at = (segs: Seg[]) => segs.map((s) => s.at);
const track = (tl: Timeline, index: number) => tl.audio.find((a) => a.index === index)!;

/** tl2 with the middle piece removed from every track (the linked delete). */
const linkedDeleted = () => deleteSegs(tl2, linkedIds(tl2, tl2.video[1].id), true);

describe('createTimeline', () => {
  it('has one full-length seg per track, all linked, with unique ids', () => {
    expect(tl0.video).toHaveLength(1);
    expect(tl0.video[0]).toMatchObject({ src: [0, 90], at: 0, link: 'L0' });
    expect(tl0.audio.map((a) => a.index)).toEqual([0, 1]);
    for (const a of tl0.audio) {
      expect(a.segs).toHaveLength(1);
      expect(a.segs[0]).toMatchObject({ src: [0, 90], at: 0, link: 'L0' });
      expect(a.gainDb).toBe(0);
      expect(a.muted).toBe(false);
    }
    const ids = [tl0.video[0].id, ...tl0.audio.map((a) => a.segs[0].id)];
    expect(new Set(ids).size).toBe(3);
    expect(ids[0]).toBe('S0');
    expect(tl0.nextId).toBe(3);
  });

  it('helpers: quantize, segLen, videoEnd, cutCount', () => {
    expect(quantize(12.004, 60)).toBe(12);
    expect(segLen(tl0.video[0])).toBe(90);
    expect(videoEnd(tl0)).toBe(90);
    expect(cutCount(tl0)).toBe(0);
    expect(cutCount(tl2)).toBe(2);
  });
});

describe('cutAt', () => {
  it('cutAt_splits_every_track', () => {
    expect(src(tl2.video)).toEqual([[0, 12], [12, 31], [31, 90]]);
    expect(at(tl2.video)).toEqual([0, 12, 31]);
    for (const a of tl2.audio) {
      expect(src(a.segs)).toEqual(src(tl2.video));
      expect(at(a.segs)).toEqual(at(tl2.video));
    }
    const [a0, a1] = tl2.audio;
    for (const i of [0, 1, 2]) {
      expect(a0.segs[i].link).toBe(tl2.video[i].link);
      expect(a1.segs[i].link).toBe(tl2.video[i].link);
    }
    const links = tl2.video.map((s) => s.link);
    expect(links[0]).toBe('L0'); // left pieces keep their link
    expect(new Set(links).size).toBe(3); // middle and right pieces are different links
    const ids = [...tl2.video, ...a0.segs, ...a1.segs].map((s) => s.id);
    expect(new Set(ids).size).toBe(9);
  });

  it('cutAt_quantizes', () => {
    expect(cutAt(tl0, 12.004).video[0].src[1]).toBe(12);
  });

  it('is a no-op on an existing boundary, at 0, and at or past the end', () => {
    expect(cutAt(tl2, 12).video).toHaveLength(3);
    expect(cutAt(tl0, 0).video).toHaveLength(1);
    expect(cutAt(tl0, 90).video).toHaveLength(1);
    expect(cutAt(tl0, 120).video).toHaveLength(1);
  });

  it('does not mutate its input', () => {
    const before = JSON.stringify(tl0);
    cutAt(tl0, 12);
    expect(JSON.stringify(tl0)).toBe(before);
  });

  it('linkedIds returns the segs that share a link on every track', () => {
    const mid = tl2.video[1];
    expect(linkedIds(tl2, mid.id).sort()).toEqual(
      [mid.id, tl2.audio[0].segs[1].id, tl2.audio[1].segs[1].id].sort(),
    );
    expect(linkedIds(tl2, 'nope')).toEqual([]);
  });
});

describe('deleteSegs', () => {
  it('linked_delete_ripples_all_tracks', () => {
    const tl = linkedDeleted();
    expect(videoEnd(tl)).toBe(71);
    expect(src(tl.video)).toEqual([[0, 12], [31, 90]]);
    expect(at(tl.video)).toEqual([0, 12]);
    for (const a of tl.audio) {
      expect(src(a.segs)).toEqual([[0, 12], [31, 90]]);
      expect(at(a.segs)).toEqual([0, 12]);
    }
  });

  it('unlinked_delete_moves_only_video', () => {
    const tl = deleteSegs(tl2, [tl2.video[1].id], false);
    expect(videoEnd(tl)).toBe(71);
    expect(src(tl.video)).toEqual([[0, 12], [31, 90]]);
    expect(src(track(tl, 0).segs)).toEqual([[0, 12], [12, 31], [31, 90]]);
    expect(at(track(tl, 0).segs)).toEqual([0, 12, 31]);
  });

  it('audio_delete_leaves_silence', () => {
    const tl = deleteSegs(tl2, [tl2.audio[1].segs[1].id], true);
    expect(src(track(tl, 1).segs)).toEqual([[0, 12], [31, 90]]);
    expect(at(track(tl, 1).segs)).toEqual([0, 31]);
    expect(tl.video).toEqual(tl2.video);
    expect(track(tl, 0).segs).toEqual(track(tl2, 0).segs);
  });

  it('last_video_segment_cannot_be_deleted', () => {
    expect(deleteSegs(tl0, [tl0.video[0].id], true)).toBe(tl0);
    expect(deleteSegs(tl0, [tl0.video[0].id], false)).toBe(tl0);
    const all = tl2.video.map((s) => s.id);
    expect(deleteSegs(tl2, all, true)).toBe(tl2);
  });

  it('linked delete of the final piece ripples nothing after it', () => {
    const tl = deleteSegs(tl2, linkedIds(tl2, tl2.video[2].id), true);
    expect(videoEnd(tl)).toBe(31);
    expect(src(track(tl, 0).segs)).toEqual([[0, 12], [12, 31]]);
  });

  it('linked delete cuts audio that spans the removed range', () => {
    // Only the video is cut, so the audio is one long seg that the delete has to split itself.
    const base = createTimeline(D, FPS, [0]);
    const videoCutOnly: Timeline = { ...base, video: cutAt(base, 20).video };
    const out = deleteSegs(videoCutOnly, [videoCutOnly.video[0].id], true);
    expect(videoEnd(out)).toBe(70);
    expect(src(out.audio[0].segs)).toEqual([[20, 90]]);
    expect(at(out.audio[0].segs)).toEqual([0]);
  });

  it('ignores ids that are not in the timeline', () => {
    expect(deleteSegs(tl2, ['nope'], true)).toBe(tl2);
  });
});

describe('moveEdge', () => {
  it('extend_video_end_linked', () => {
    const base = linkedDeleted();
    const tl = moveEdge(base, base.video[0].id, 'end', 14, true);
    expect(src(tl.video)).toEqual([[0, 14], [31, 90]]);
    expect(at(tl.video)).toEqual([0, 14]);
    expect(src(track(tl, 0).segs)).toEqual([[0, 14], [31, 90]]);
    expect(at(track(tl, 0).segs)).toEqual([0, 14]);
    expect(src(track(tl, 1).segs)).toEqual([[0, 14], [31, 90]]);
    expect(videoEnd(tl)).toBe(73);
  });

  it('video end edge, unlinked, leaves audio alone', () => {
    const base = linkedDeleted();
    const tl = moveEdge(base, base.video[0].id, 'end', 14, false);
    expect(src(tl.video)).toEqual([[0, 14], [31, 90]]);
    expect(src(track(tl, 0).segs)).toEqual([[0, 12], [31, 90]]);
    expect(at(track(tl, 0).segs)).toEqual([0, 12]);
  });

  it('shrinking a video end edge ripples later video and linked audio left', () => {
    const base = linkedDeleted();
    const tl = moveEdge(base, base.video[0].id, 'end', 10, true);
    expect(src(tl.video)).toEqual([[0, 10], [31, 90]]);
    expect(at(tl.video)).toEqual([0, 10]);
    expect(src(track(tl, 0).segs)).toEqual([[0, 10], [31, 90]]);
    expect(at(track(tl, 0).segs)).toEqual([0, 10]);
  });

  it('extend_clamps_to_source', () => {
    const last = tl2.video[2];
    const tl = moveEdge(tl2, last.id, 'end', 200, true);
    expect(tl.video[2].src[1]).toBe(90);
    expect(tl.audio[0].segs[2].src[1]).toBe(90);
  });

  it('video start edge keeps at, changes src[0], and ripples what follows', () => {
    // Trim 2 s off the front of the second piece [12,31]@12 -> [14,31]@12.
    const tl = moveEdge(tl2, tl2.video[1].id, 'start', 14, true);
    expect(src(tl.video)).toEqual([[0, 12], [14, 31], [31, 90]]);
    expect(at(tl.video)).toEqual([0, 12, 29]);
    expect(src(track(tl, 0).segs)).toEqual([[0, 12], [14, 31], [31, 90]]);
    expect(at(track(tl, 0).segs)).toEqual([0, 12, 29]);
  });

  it('video start edge restores material to the left and clamps at source 0', () => {
    const base = linkedDeleted(); // second piece is [31,90]@12
    const ext = moveEdge(base, base.video[1].id, 'start', 10, true);
    expect(src(ext.video)).toEqual([[0, 12], [29, 90]]);
    expect(at(ext.video)).toEqual([0, 12]);
    const clamped = moveEdge(tl2, tl2.video[0].id, 'start', -5, true);
    expect(clamped.video[0].src[0]).toBe(0);
  });

  it('keeps at least one frame', () => {
    const tl = moveEdge(tl2, tl2.video[1].id, 'end', 5, true);
    expect(segLen(tl.video[1])).toBeCloseTo(1 / FPS, 9);
    const tl2s = moveEdge(tl2, tl2.video[1].id, 'start', 50, true);
    expect(segLen(tl2s.video[1])).toBeCloseTo(1 / FPS, 9);
  });

  it('quantizes toT', () => {
    const tl = moveEdge(tl2, tl2.video[2].id, 'end', 80.004, true);
    expect(tl.video[2].src[1]).toBe(80);
  });

  it('audio_edge_stops_at_neighbor', () => {
    const gap = deleteSegs(tl2, [tl2.audio[1].segs[1].id], true);
    const first = track(gap, 1).segs[0];
    const tl = moveEdge(gap, first.id, 'end', 40, true);
    expect(track(tl, 1).segs[0].src[1]).toBe(31);
    expect(track(tl, 1).segs[0].at).toBe(0);
    expect(tl.video).toEqual(gap.video);
  });

  it('audio start edge moves at and src[0] together and stops at its left neighbor and at 0', () => {
    const gap = deleteSegs(tl2, [tl2.audio[1].segs[1].id], true);
    const last = track(gap, 1).segs[1]; // [31,90]@31, neighbor ends at 12
    const left = moveEdge(gap, last.id, 'start', 5, true);
    expect(track(left, 1).segs[1].at).toBe(12);
    expect(track(left, 1).segs[1].src[0]).toBe(12);
    const right = moveEdge(gap, last.id, 'start', 40, true);
    expect(track(right, 1).segs[1].at).toBe(40);
    expect(track(right, 1).segs[1].src[0]).toBe(40);
    const first = track(gap, 1).segs[0];
    const atZero = moveEdge(gap, first.id, 'start', -3, true);
    expect(track(atZero, 1).segs[0].at).toBe(0);
    expect(track(atZero, 1).segs[0].src[0]).toBe(0);
  });

  it('audio edges only touch their own track', () => {
    const tl = moveEdge(tl2, tl2.audio[0].segs[2].id, 'end', 100, true);
    expect(track(tl, 0).segs[2].src[1]).toBe(90);
    expect(tl.video).toEqual(tl2.video);
    expect(track(tl, 1).segs).toEqual(track(tl2, 1).segs);
  });

  it('returns the same timeline when nothing changes or the id is unknown', () => {
    expect(moveEdge(tl2, tl2.video[2].id, 'end', 90, true)).toBe(tl2);
    expect(moveEdge(tl2, 'nope', 'end', 10, true)).toBe(tl2);
  });
});

describe('gain, mute', () => {
  it('setGain clamps to [-60, 6]', () => {
    expect(track(setGain(tl0, 1, -3), 1).gainDb).toBe(-3);
    expect(track(setGain(tl0, 1, -100), 1).gainDb).toBe(-60);
    expect(track(setGain(tl0, 1, 20), 1).gainDb).toBe(6);
    expect(track(setGain(tl0, 1, -3), 0).gainDb).toBe(0);
  });
  it('setMuted flips only that track', () => {
    const tl = setMuted(tl0, 0, true);
    expect(track(tl, 0).muted).toBe(true);
    expect(track(tl, 1).muted).toBe(false);
  });
});

describe('snapping', () => {
  it('snap_picks_nearest_within_threshold', () => {
    expect(snap(12.05, [12, 31], 0.1)).toBe(12);
    expect(snap(12.5, [12], 0.1)).toBe(12.5);
    expect(snap(20, [12, 21, 31], 2)).toBe(21);
    expect(snap(5, [], 1)).toBe(5);
  });

  it('snapCandidates: playhead, every edge on every track, video end', () => {
    const c = snapCandidates(tl2, 50);
    for (const v of [0, 12, 31, 90, 50]) expect(c).toContain(v);
    // an edge that exists only on an audio track is still a candidate
    const gap = deleteSegs(tl2, [tl2.audio[1].segs[1].id], true);
    const cg = snapCandidates(gap, 0);
    expect(cg).toContain(12);
    expect(cg).toContain(31);
  });

  it('snapCandidates honours exclude', () => {
    // One audio seg [31,90]@31 whose start is not shared with any other edge.
    const base = createTimeline(D, FPS, [0]);
    const cut = cutAt(base, 31);
    const solo = deleteSegs({ ...base, audio: cut.audio }, [cut.audio[0].segs[0].id], false);
    const seg = solo.audio[0].segs[0];
    expect(seg.at).toBe(31);
    expect(snapCandidates(solo, 0)).toContain(31);
    expect(snapCandidates(solo, 0, { segId: seg.id, edge: 'start' })).not.toContain(31);
    expect(snapCandidates(solo, 0, { segId: seg.id, edge: 'end' })).toContain(31);
  });
});

/** No track holds two segs with one link, and no track has overlapping segs. */
function expectInvariants(tl: Timeline) {
  const tracks = [tl.video, ...tl.audio.map((a) => a.segs)];
  for (const segs of tracks) {
    const sorted = [...segs].sort((a, b) => a.at - b.at);
    expect(new Set(segs.map((s) => s.link)).size).toBe(segs.length);
    for (let i = 1; i < sorted.length; i++) {
      expect(sorted[i].at).toBeGreaterThanOrEqual(sorted[i - 1].at + segLen(sorted[i - 1]) - 1e-9);
    }
  }
}

describe('link and overlap invariants', () => {
  /** Reviewer's repro: audio spans a whole video range that is then linked-deleted. */
  const spanning = () => {
    let tl = createTimeline(D, FPS, [0]);
    tl = cutAt(cutAt(tl, 12), 31);
    tl = deleteSegs(tl, [tl.audio[0].segs[1].id], true); // silence 12..31 on A0
    tl = cutAt(tl, 20); // splits video only: A0 has no material there
    const first = tl.audio[0].segs[0];
    tl = moveEdge(tl, first.id, 'end', 31, true); // A0 now one seg [0,31] over three video segs
    expect(tl.audio[0].segs.map((s) => s.src)).toEqual([[0, 31], [31, 90]]);
    return tl;
  };

  it('linked delete over a spanning audio seg gives the right remnant a fresh link', () => {
    const tl = spanning();
    const out = deleteSegs(tl, [tl.video[1].id], true); // video [12,20]
    expect(src(out.audio[0].segs)).toEqual([[0, 12], [20, 31], [31, 90]]);
    expect(at(out.audio[0].segs)).toEqual([0, 12, 23]);
    expectInvariants(out);
    const ids = linkedIds(out, out.video[0].id);
    expect(ids).toContain(out.audio[0].segs[0].id);
    expect(ids).not.toContain(out.audio[0].segs[1].id);
  });

  it('a linked video end drag after that delete moves only the audio under the dragged edge', () => {
    const tl = spanning();
    const out = deleteSegs(tl, [tl.video[1].id], true);
    const dragged = moveEdge(out, out.video[0].id, 'end', 14, true);
    expect(src(dragged.audio[0].segs)).toEqual([[0, 14], [20, 31], [31, 90]]);
    expect(at(dragged.audio[0].segs)).toEqual([0, 14, 25]);
    expectInvariants(dragged);
  });

  it('linked drag leaves linked audio whose edge is not on the dragged video edge alone', () => {
    // A1 is desynced: its first seg runs to 15 while the video cut is at 12.
    let tl = createTimeline(D, FPS, [0, 1]);
    tl = cutAt(tl, 12);
    tl = deleteSegs(tl, [tl.audio[1].segs[1].id], false); // silence after 12 on A1
    tl = moveEdge(tl, tl.audio[1].segs[0].id, 'end', 15, false);
    expect(track(tl, 1).segs[0].src).toEqual([0, 15]);
    const out = moveEdge(tl, tl.video[0].id, 'end', 14, true);
    expect(track(out, 0).segs[0].src).toEqual([0, 14]); // lines up with the video edge
    expect(track(out, 0).segs[1].at).toBe(14); // and its track ripples
    expect(track(out, 1).segs[0].src).toEqual([0, 15]); // does not line up: untouched
    expectInvariants(out);
  });

  it('non-adjacent linked deletes and a cut afterwards keep every track in step', () => {
    let tl = createTimeline(D, FPS, [0, 1]);
    for (const t of [10, 20, 30, 40, 50]) tl = cutAt(tl, t);
    tl = deleteSegs(tl, [tl.video[1].id, tl.video[3].id], true);
    expect(src(tl.video)).toEqual([[0, 10], [20, 30], [40, 50], [50, 90]]);
    expect(videoEnd(tl)).toBe(70);
    for (const a of tl.audio) {
      expect(src(a.segs)).toEqual(src(tl.video));
      expect(at(a.segs)).toEqual(at(tl.video));
    }
    tl = cutAt(tl, 25);
    expect(tl.audio[0].segs[2].link).toBe(tl.video[2].link);
    expect(tl.audio[1].segs[2].link).toBe(tl.video[2].link);
    expectInvariants(tl);
  });
});

describe('linked edge drags on desynced audio never overlap', () => {
  it('shrinking a video end edge does not pull later audio over a seg that stays put', () => {
    let tl = cutAt(createTimeline(D, FPS, [0, 1]), 12);
    tl = moveEdge(tl, tl.audio[1].segs[1].id, 'start', 15, false); // A1 [15,90]@15
    tl = moveEdge(tl, tl.audio[1].segs[0].id, 'end', 15, false); // A1 [0,15]
    expect(at(track(tl, 1).segs)).toEqual([0, 15]);
    const out = moveEdge(tl, tl.video[0].id, 'end', 8, true);
    expect(src(out.video)).toEqual([[0, 8], [12, 90]]);
    expect(src(track(out, 0).segs)).toEqual([[0, 8], [12, 90]]); // aligned track ripples
    expect(at(track(out, 0).segs)).toEqual([0, 8]);
    expect(at(track(out, 1).segs)).toEqual([0, 15]); // desynced track cannot close up
    expectInvariants(out);
  });

  it('a start-edge extend that lengthens a shorter aligned seg pushes its neighbour along', () => {
    let tl = cutAt(cutAt(createTimeline(D, FPS, [0, 1]), 12), 40);
    const a1 = tl.audio[1].segs;
    tl = moveEdge(tl, a1[1].id, 'end', 30, false); // A1 [12,30]@12
    tl = moveEdge(tl, a1[2].id, 'start', 31, false); // A1 [31,90]@31
    expect(at(track(tl, 1).segs)).toEqual([0, 12, 31]);
    const out = moveEdge(tl, tl.video[1].id, 'start', 10, true); // video [10,40]@12
    expect(src(track(out, 1).segs)).toEqual([[0, 12], [10, 30], [31, 90]]);
    expect(at(track(out, 1).segs)).toEqual([0, 12, 33]);
    expectInvariants(out);
    const shrunk = moveEdge(tl, tl.video[1].id, 'start', 20, true); // video [20,40]@12
    expect(src(track(shrunk, 1).segs)[1]).toEqual([20, 30]);
    expect(at(track(shrunk, 1).segs)).toEqual([0, 12, 23]);
    expectInvariants(shrunk);
  });
});

describe('randomized edit sequences keep the invariants', () => {
  // mulberry32: small seeded PRNG, so a failure reproduces from the printed seed.
  const prng = (seed: number) => () => {
    seed = (seed + 0x6d2b79f5) | 0;
    let t = Math.imul(seed ^ (seed >>> 15), 1 | seed);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  const TOL = 1e-6;

  function check(tl: Timeline, where: string) {
    const fail = (msg: string) => {
      throw new Error(`${where}: ${msg}\n${JSON.stringify(tl)}`);
    };
    let t = 0;
    for (const s of tl.video) {
      if (Math.abs(s.at - t) > TOL) fail(`video not contiguous at ${s.id}`);
      t += segLen(s);
    }
    const tracks = [tl.video, ...tl.audio.map((a) => a.segs)];
    for (const segs of tracks) {
      const sorted = [...segs].sort((a, b) => a.at - b.at);
      if (new Set(segs.map((s) => s.link)).size !== segs.length) fail('shared link on one track');
      if (new Set(segs.map((s) => s.id)).size !== segs.length) fail('duplicate id');
      for (let i = 0; i < sorted.length; i++) {
        const x = sorted[i];
        if (x.at < -TOL) fail(`negative at ${x.id}`);
        if (x.src[0] < -TOL || x.src[1] > tl.sourceDuration + TOL) fail(`src out of range ${x.id}`);
        if (segLen(x) < 1 / tl.fps - TOL) fail(`shorter than a frame ${x.id}`);
        if (i > 0 && x.at < sorted[i - 1].at + segLen(sorted[i - 1]) - TOL) fail(`overlap at ${x.id}`);
      }
    }
  }

  it('holds across 300 seeded sequences of cuts, deletes and edge drags', () => {
    for (let seed = 1; seed <= 300; seed++) {
      const rnd = prng(seed);
      const pick = <T,>(xs: T[]): T => xs[Math.floor(rnd() * xs.length)];
      let tl = createTimeline(D, FPS, [0, 1]);
      for (let step = 0; step < 30; step++) {
        const allSegs = [...tl.video, ...tl.audio.flatMap((a) => a.segs)];
        const linked = rnd() < 0.6;
        const r = rnd();
        let label: string;
        if (r < 0.25) {
          const t = rnd() * (videoEnd(tl) + 5);
          label = `cutAt(${t})`;
          tl = cutAt(tl, t);
        } else if (r < 0.45) {
          const target = pick(allSegs);
          const ids = rnd() < 0.5 ? linkedIds(tl, target.id) : [target.id];
          label = `deleteSegs(${ids.join(',')}, ${linked})`;
          tl = deleteSegs(tl, ids, linked);
        } else {
          const target = pick(allSegs);
          const edge = rnd() < 0.5 ? 'start' : 'end';
          const to = rnd() * (D + 20) - 5;
          label = `moveEdge(${target.id}, ${edge}, ${to}, ${linked})`;
          tl = moveEdge(tl, target.id, edge, to, linked);
        }
        check(tl, `seed ${seed} step ${step} ${label}`);
      }
    }
  });
});

describe('snapCandidates while dragging a video edge', () => {
  it('drops the video boundaries and video end that move with an end drag', () => {
    const tl = cutAt(cutAt(createTimeline(D, FPS, []), 12), 31);
    const c = snapCandidates(tl, 5, { segId: tl.video[0].id, edge: 'end' });
    expect(c.sort((x, y) => x - y)).toEqual([0, 5]);
  });

  it('a start drag drops boundaries from the dragged position on', () => {
    const tl = cutAt(cutAt(createTimeline(D, FPS, []), 12), 31);
    const c = snapCandidates(tl, 5, { segId: tl.video[1].id, edge: 'start' });
    expect(c.sort((x, y) => x - y)).toEqual([0, 5]);
  });

  it('drops same-side edges of linked audio but keeps those of other segs', () => {
    let tl = cutAt(createTimeline(D, FPS, [0]), 12);
    tl = deleteSegs(tl, [tl.audio[0].segs[1].id], false);
    tl = moveEdge(tl, tl.audio[0].segs[0].id, 'end', 15, false); // A0 [0,15], link of video seg 0
    expect(snapCandidates(tl, 0)).toContain(15);
    expect(snapCandidates(tl, 0, { segId: tl.video[0].id, edge: 'end' })).not.toContain(15);
    // Dragging the other video seg's end: 15 belongs to a different link and stays.
    expect(snapCandidates(tl, 0, { segId: tl.video[1].id, edge: 'end' })).toContain(15);
  });
});

describe('snapCandidates and the linked flag', () => {
  // Cut at 12: the right piece of the cut on A0 starts at 12 with a different link than video seg 0.
  const cut12 = cutAt(createTimeline(D, FPS, [0]), 12);
  const dragEnd = { segId: cut12.video[0].id, edge: 'end' as const };

  it('linked: audio edges at or after the dragged edge ripple with it, so they are dropped', () => {
    const c = snapCandidates(cut12, 5, dragEnd, true);
    expect([...new Set(c)].sort((x, y) => x - y)).toEqual([0, 5]);
    expect(snapCandidates(cut12, 5, dragEnd)).toEqual(snapCandidates(cut12, 5, dragEnd, true));
  });

  it('unlinked: audio does not move, so its edges (including the cut piece) stay targets', () => {
    const c = snapCandidates(cut12, 5, dragEnd, false);
    expect(c).toContain(12); // A0's first end and its right piece's start
    expect(c).toContain(90); // A0's last end
    expect(c).toContain(0);
    expect(c).toContain(5);
  });

  it('linked: audio edges before the dragged edge stay targets', () => {
    const tl = cutAt(cutAt(createTimeline(D, FPS, [0]), 12), 31);
    const c = snapCandidates(tl, 0, { segId: tl.video[2].id, edge: 'end' }, true);
    expect(c).toContain(12);
    expect(c).toContain(31);
    expect(c).not.toContain(90);
  });
});

describe('export request', () => {
  it('toExportRequest lists video pieces and per-track audio', () => {
    const tl = linkedDeleted();
    const req = toExportRequest(tl, 7, 'C:\\clips\\a.mp4', 'quick', true);
    expect(req).toMatchObject({
      session: 7,
      input: 'C:\\clips\\a.mp4',
      mode: 'quick',
      fit_20mb: true,
      duration_s: 71,
    });
    expect(req.video).toEqual([
      { src_start: 0, src_end: 12 },
      { src_start: 31, src_end: 90 },
    ]);
    expect(req.audio.map((a) => a.index)).toEqual([0, 1]);
    expect(req.audio[0]).toEqual({
      index: 0,
      gain_db: 0,
      pieces: [
        { at: 0, src_start: 0, src_end: 12 },
        { at: 12, src_start: 31, src_end: 90 },
      ],
    });
  });

  it('export audio mode sends no video', () => {
    const req = toExportRequest(tl2, 1, 'x.mp4', 'audio', false);
    expect(req.video).toEqual([]);
    expect(req.audio).toHaveLength(2);
    expect(req.duration_s).toBe(90);
  });

  it('export_request_clips_and_skips_muted', () => {
    // Video is shortened to 50 s without touching audio, so A0's [31,90]@31 runs past the end.
    let tl = moveEdge(tl2, tl2.video[2].id, 'end', 50, false);
    expect(videoEnd(tl)).toBe(50);
    tl = setMuted(tl, 1, true);
    const req = toExportRequest(tl, 1, 'x.mp4', 'quick', false);
    expect(req.audio).toHaveLength(1);
    expect(req.audio[0].index).toBe(0);
    const pieces = req.audio[0].pieces;
    expect(pieces).toHaveLength(3);
    const last = pieces[pieces.length - 1];
    expect(last.at).toBe(31);
    expect(last.src_start).toBe(31);
    expect(last.at + (last.src_end - last.src_start)).toBeCloseTo(50, 9);
    expect(req.duration_s).toBe(50);
  });

  it('a fader at −∞ (MIN_GAIN_DB or below) is skipped like a mute', () => {
    const silent = setGain(tl2, 1, MIN_GAIN_DB);
    expect(toExportRequest(silent, 1, 'x.mp4', 'quick', false).audio.map((a) => a.index)).toEqual([0]);
    expect(toExportRequest(silent, 1, 'x.mp4', 'audio', false).audio.map((a) => a.index)).toEqual([0]);
    // One step above the bottom is still exported, at its gain.
    const quiet = setGain(tl2, 1, MIN_GAIN_DB + 0.5);
    expect(toExportRequest(quiet, 1, 'x.mp4', 'quick', false).audio.map((a) => [a.index, a.gain_db])).toEqual([
      [0, 0],
      [1, MIN_GAIN_DB + 0.5],
    ]);
  });

  it('drops pieces that start at or after the video end and tracks left with none', () => {
    // Video is [0,12] only; A0 is [0,12]@0 and [12,90]@12 and so on.
    const tl = deleteSegs(tl2, [tl2.video[1].id, tl2.video[2].id], false);
    expect(videoEnd(tl)).toBe(12);
    const req = toExportRequest(tl, 1, 'x.mp4', 'quick', false);
    expect(req.audio[0].pieces).toEqual([{ at: 0, src_start: 0, src_end: 12 }]);
    // Remove A1's first piece: A1 has material only after the end, so the track is dropped.
    const gone = deleteSegs(tl, [tl.audio[1].segs[0].id], false);
    expect(toExportRequest(gone, 1, 'x.mp4', 'quick', false).audio.map((a) => a.index)).toEqual([0]);
  });

  it('hasAudibleAudio', () => {
    expect(hasAudibleAudio(tl0)).toBe(true);
    const oneMuted = setMuted(tl0, 0, true);
    expect(hasAudibleAudio(oneMuted)).toBe(true);
    expect(hasAudibleAudio(setMuted(oneMuted, 1, true))).toBe(false);
    expect(hasAudibleAudio(createTimeline(D, FPS, []))).toBe(false);
  });

  it('hasAudibleAudio counts a fader at −∞ as silent', () => {
    const oneDown = setGain(tl0, 0, MIN_GAIN_DB);
    expect(hasAudibleAudio(oneDown)).toBe(true);
    expect(hasAudibleAudio(setGain(oneDown, 1, MIN_GAIN_DB))).toBe(false);
    expect(hasAudibleAudio(setMuted(oneDown, 1, true))).toBe(false);
    expect(hasAudibleAudio(setGain(oneDown, 1, MIN_GAIN_DB + 0.5))).toBe(true);
  });

  it('hasAudibleAudio ignores material that is entirely past the video end', () => {
    let tl = deleteSegs(tl2, [tl2.video[1].id, tl2.video[2].id], false); // video ends at 12
    tl = deleteSegs(tl, [tl.audio[0].segs[0].id, tl.audio[1].segs[0].id], false);
    expect(hasAudibleAudio(tl)).toBe(false);
  });
});
