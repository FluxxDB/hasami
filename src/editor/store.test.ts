import { describe, expect, it } from 'vitest';
import type { EditorMedia } from './api';
import { EditorStore } from './store.svelte';
import { cutAt, cutCount, deleteSegs, segLen, videoEnd } from './timeline';

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

describe('EditorStore commands', () => {
  const loaded = () => {
    const s = new EditorStore();
    s.load(media());
    return s;
  };

  it('cut at playhead 12 adds one cut on every track', () => {
    const s = loaded();
    s.seek(12);
    s.command('cut');
    expect(cutCount(s.timeline)).toBe(1);
    expect(s.timeline.video[1].at).toBe(12);
    expect(s.timeline.audio.every((a) => a.segs.length === 2)).toBe(true);
    expect(s.history.past).toHaveLength(1);
  });

  it('delete removes a linked selection and ripples', () => {
    const s = loaded();
    s.seek(12);
    s.command('cut');
    s.seek(31);
    s.command('cut');
    const mid = s.timeline.video[1];
    s.selectAt(mid.id);
    expect(s.selection).toHaveLength(3); // V1 + A1 + A2 pieces made by the same cuts
    s.command('delete');
    expect(videoEnd(s.timeline)).toBeCloseTo(90 - 19, 9);
    expect(s.timeline.video.map((v) => v.src)).toEqual([
      [0, 12],
      [31, 90],
    ]);
    for (const a of s.timeline.audio) expect(a.segs.map((x) => x.at)).toEqual([0, 12]);
    expect(s.selection).toEqual([]);
  });

  it('delete with Linked off removes only the clicked audio piece, leaving silence', () => {
    const s = loaded();
    s.seek(12);
    s.command('cut');
    s.command('toggleLinked');
    const a1 = s.timeline.audio[0].segs[1];
    s.selectAt(a1.id);
    expect(s.selection).toEqual([a1.id]);
    s.command('delete');
    expect(videoEnd(s.timeline)).toBe(90);
    expect(s.timeline.audio[0].segs).toHaveLength(1);
    expect(s.timeline.audio[1].segs).toHaveLength(2);
  });

  it('undo and redo restore the timeline', () => {
    const s = loaded();
    const before = s.timeline;
    s.seek(12);
    s.command('cut');
    const after = s.timeline;
    s.command('undo');
    expect(s.timeline).toBe(before);
    expect(s.edited).toBe(false);
    s.command('redo');
    expect(s.timeline).toBe(after);
  });

  it('an edit that shortens the video pulls the playhead inside it', () => {
    const s = loaded();
    s.seek(60);
    s.command('cut');
    s.seek(80);
    s.selectAt(s.timeline.video[1].id);
    s.command('delete');
    expect(s.playhead).toBe(60);
    s.command('undo');
    expect(s.playhead).toBe(60);
    expect(videoEnd(s.timeline)).toBe(90);
  });

  it('toggleSnap and toggleLinked flip the flags', () => {
    const s = loaded();
    expect(s.snapping).toBe(true);
    expect(s.linked).toBe(true);
    s.command('toggleSnap');
    s.command('toggleLinked');
    expect(s.snapping).toBe(false);
    expect(s.linked).toBe(false);
    s.command('toggleSnap');
    expect(s.snapping).toBe(true);
  });

  it('frame steps move by one frame and seconds by one second', () => {
    const s = loaded();
    s.command('frameFwd');
    expect(s.playhead).toBeCloseTo(1 / 60, 12);
    s.command('frameBack');
    s.command('frameBack');
    expect(s.playhead).toBe(0);
    s.command('secFwd');
    expect(s.playhead).toBeCloseTo(1, 12);
  });

  it('nextCut and prevCut jump between cut points; home and end go to the ends', () => {
    const s = loaded();
    s.seek(12);
    s.command('cut');
    s.seek(31);
    s.command('cut');
    s.command('home');
    s.command('nextCut');
    expect(s.playhead).toBe(12);
    s.command('nextCut');
    expect(s.playhead).toBe(31);
    s.command('prevCut');
    expect(s.playhead).toBe(12);
    s.command('end');
    expect(s.playhead).toBe(90);
  });

  it('seek is quantized to the frame grid and clamped to the video', () => {
    const s = loaded();
    s.seek(10.004);
    expect(s.playhead).toBeCloseTo(10, 12);
    s.seek(-3);
    expect(s.playhead).toBe(0);
    s.seek(500);
    expect(s.playhead).toBe(90);
  });

  it('an edge drag previews live and records exactly one history entry', () => {
    const s = loaded();
    s.seek(44);
    s.command('cut');
    s.selectAt(s.timeline.video[1].id);
    s.command('delete'); // keep 0–44
    const steps = s.history.past.length;
    const v0 = s.timeline.video[0].id;
    s.snapping = false;
    s.beginEdgeDrag(v0, 'end');
    s.dragEdgeTo(50);
    expect(videoEnd(s.timeline)).toBeCloseTo(50, 9);
    s.dragEdgeTo(60);
    s.dragEdgeTo(70);
    expect(s.history.past.length).toBe(steps); // nothing recorded mid-drag
    s.endEdgeDrag();
    expect(s.history.past.length).toBe(steps + 1);
    expect(videoEnd(s.timeline)).toBeCloseTo(70, 9);
    // Linked: the audio pieces that ended with the video came back with it.
    for (const a of s.timeline.audio) expect(Math.max(...a.segs.map((x) => x.at + segLen(x)))).toBeCloseTo(70, 9);
    s.command('undo');
    expect(videoEnd(s.timeline)).toBeCloseTo(44, 9);
  });

  it('an edge drag that ends where it started records nothing', () => {
    const s = loaded();
    s.seek(30);
    s.command('cut');
    const steps = s.history.past.length;
    s.beginEdgeDrag(s.timeline.video[0].id, 'end');
    s.dragEdgeTo(20);
    s.dragEdgeTo(30);
    s.endEdgeDrag();
    expect(s.history.past.length).toBe(steps);
  });

  it('an edge drag snaps to the playhead within SNAP_PX and shows where', () => {
    const s = loaded();
    s.seek(44);
    s.command('cut');
    s.selectAt(s.timeline.video[1].id);
    s.command('delete');
    s.seek(20);
    s.pxPerSecond = 20; // SNAP_PX = 8 → 0.4 s
    s.beginEdgeDrag(s.timeline.audio[1].segs[0].id, 'end');
    s.dragEdgeTo(20.3);
    expect(s.snapAt).toBe(20);
    expect(s.timeline.audio[1].segs[0].src[1]).toBe(20);
    s.dragEdgeTo(25);
    expect(s.snapAt).toBeNull();
    s.endEdgeDrag();
    expect(s.snapAt).toBeNull();
  });

  it('setGain previews without history until it commits', () => {
    const s = loaded();
    s.setGain(0, -6, false);
    expect(s.timeline.audio[0].gainDb).toBe(-6);
    expect(s.history.past).toHaveLength(0);
    s.setGain(0, -6, true);
    expect(s.history.past).toHaveLength(1);
    expect(s.timeline.audio[0].gainDb).toBe(-6);
  });

  it('a slider that returns to where it started records nothing', () => {
    const s = loaded();
    s.setGain(0, -12, false);
    s.setGain(0, 0, false);
    s.setGain(0, 0, true);
    expect(s.history.past).toHaveLength(0);
  });

  it('toggleMute is an undo step', () => {
    const s = loaded();
    s.toggleMute(2);
    expect(s.timeline.audio[1].muted).toBe(true);
    expect(s.history.past).toHaveLength(1);
    s.toggleMute(2);
    expect(s.timeline.audio[1].muted).toBe(false);
    s.command('undo');
    expect(s.timeline.audio[1].muted).toBe(true);
  });

  it('L taps speed up, J reverses at 1x, K stops; Space plays and pauses', () => {
    const s = loaded();
    s.command('L');
    expect(s.shuttle).toEqual({ dir: 1, speed: 1 });
    s.command('L');
    expect(s.shuttle).toEqual({ dir: 1, speed: 2 });
    s.command('L');
    expect(s.shuttle).toEqual({ dir: 1, speed: 4 });
    s.command('J');
    expect(s.shuttle).toEqual({ dir: -1, speed: 1 });
    s.command('K');
    expect(s.shuttle).toEqual({ dir: 0, speed: 1 });
    s.command('playPause');
    expect(s.shuttle).toEqual({ dir: 1, speed: 1 });
    s.command('playPause');
    expect(s.shuttle.dir).toBe(0);
  });

  it('Space at the end of the video plays from the start', () => {
    const s = loaded();
    s.command('end');
    s.command('playPause');
    expect(s.playhead).toBe(0);
    expect(s.shuttle.dir).toBe(1);
  });

  it('frame steps stop shuttling', () => {
    const s = loaded();
    s.command('L');
    s.command('frameFwd');
    expect(s.shuttle.dir).toBe(0);
  });

  it('selectAt honours Linked', () => {
    const s = loaded();
    s.seek(12);
    s.command('cut');
    const v = s.timeline.video[1];
    s.selectAt(v.id);
    expect(new Set(s.selection)).toEqual(new Set([v.id, s.timeline.audio[0].segs[1].id, s.timeline.audio[1].segs[1].id]));
    s.linked = false;
    s.selectAt(v.id);
    expect(s.selection).toEqual([v.id]);
  });

  it('wheel zoom keeps the time under the cursor, and pans never go before 0', () => {
    const s = loaded();
    s.viewWidth = 1000;
    s.pxPerSecond = 20;
    s.scrollX = 100;
    const t = (300 + s.scrollX) / s.pxPerSecond;
    s.zoomAt(2, 300);
    expect(s.pxPerSecond).toBe(40);
    expect((300 + s.scrollX) / s.pxPerSecond).toBeCloseTo(t, 9);
    s.pan(-1e6);
    expect(s.scrollX).toBe(0);
  });

  it('seeking off screen scrolls the playhead into view', () => {
    const s = loaded();
    s.viewWidth = 400;
    s.pxPerSecond = 20; // 20 s visible
    s.seek(60);
    const x = s.playhead * s.pxPerSecond - s.scrollX;
    expect(x).toBeGreaterThanOrEqual(0);
    expect(x).toBeLessThanOrEqual(400);
  });
});

describe('EditorStore seek requests', () => {
  it('every seek is a new request, even to the time the engine last reported', () => {
    const s = new EditorStore();
    s.load(media());
    s.playhead = 90; // the engine reporting that playback ran to the end
    const before = s.seekSeq;
    s.command('home');
    s.command('end');
    expect(s.playhead).toBe(90);
    expect(s.seekSeq).toBe(before + 2);
  });

  it('a cut mid-drag records the drag and the cut as separate undo steps', () => {
    const s = new EditorStore();
    s.load(media());
    s.seek(44);
    s.command('cut');
    s.snapping = false;
    const steps = s.history.past.length;
    s.beginEdgeDrag(s.timeline.video[0].id, 'end');
    s.dragEdgeTo(40);
    s.seek(20);
    s.command('cut');
    expect(s.history.past.length).toBe(steps + 2);
    s.command('undo');
    expect(cutCount(s.timeline)).toBe(1);
    expect(s.timeline.video[0].src[1]).toBeCloseTo(40, 9);
  });
});
