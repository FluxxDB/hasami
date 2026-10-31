import { describe, expect, it } from 'vitest';
import {
  createTimeline,
  cutAt,
  cutCount,
  deleteSegs,
  hasAudibleAudio,
  linkedIds,
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

  it('hasAudibleAudio ignores material that is entirely past the video end', () => {
    let tl = deleteSegs(tl2, [tl2.video[1].id, tl2.video[2].id], false); // video ends at 12
    tl = deleteSegs(tl, [tl.audio[0].segs[0].id, tl.audio[1].segs[0].id], false);
    expect(hasAudibleAudio(tl)).toBe(false);
  });
});
