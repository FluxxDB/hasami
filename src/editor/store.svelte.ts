// The Quick Trim editor's state. Components render from it; edits go through `edit()` so every
// one is an undo step. The timeline itself lives in `history.present`, except while an edge drag
// or a volume slider is moving: that live state is `preview`, and it becomes one undo step when
// the drag or the slider is released.

import type { EditorMedia, Peaks, Thumb } from './api';
import { SNAP_PX, zoomAround } from './geometry';
import { initHistory, isEdited, push, redo, undo, type History } from './history';
import type { Command } from './keys';
import { STOPPED, nextCut, prevCut, shuttle, stepFrames, type Shuttle } from './playbackModel';
import {
  createTimeline,
  cutAt,
  deleteSegs,
  linkedIds,
  moveEdge,
  quantize,
  segLen,
  setGain,
  setMuted,
  snap,
  snapCandidates,
  videoEnd,
  type Timeline,
} from './timeline';

const DEFAULT_FPS = 30; // for audio-only files, which have no frame rate of their own
const PLAY_1X: Shuttle = { dir: 1, speed: 1 };
/** Room kept after the end of the material when panning, so the last edge stays grabbable. */
const PAN_MARGIN_PX = 24;
/** How near the lanes' right edge the playhead may get while playing before the view pages. */
const FOLLOW_MARGIN_PX = 24;

const clamp = (x: number, lo: number, hi: number): number => Math.min(Math.max(x, lo), hi);

export class EditorStore {
  // Raw (not deeply reactive): history snapshots are compared by identity and never mutated.
  media = $state.raw<EditorMedia | null>(null);
  history = $state.raw<History<Timeline>>(initHistory(createTimeline(0, DEFAULT_FPS, [])));
  /** The live timeline during an edge drag or a volume slide; null otherwise. */
  preview = $state.raw<Timeline | null>(null);
  selection = $state.raw<string[]>([]);
  playhead = $state(0);

  pxPerSecond = $state(20);
  scrollX = $state(0);
  /** Width of the lanes in px, measured by the window (0 until laid out). */
  viewWidth = $state(0);
  /** Set by `load`; the window fits the clip to the lanes once it knows their width. */
  needsFit = $state(false);

  linked = $state(true);
  snapping = $state(true);
  fit20 = $state(false);
  showEmpty = $state(false);

  peaks = $state.raw<Record<number, Peaks>>({});
  thumbs = $state.raw<Thumb[]>([]);

  shuttle = $state.raw<Shuttle>(STOPPED);
  /** Where the dragged edge snapped to (drawn as a 1px line); null when it isn't snapped. */
  snapAt = $state<number | null>(null);
  /** Progress (0..1) of the H.264 preview proxy while it's being made; null otherwise. */
  proxyFraction = $state<number | null>(null);

  queue = $state<{ files: string[]; index: number }>({ files: [], index: 0 });
  exports = $state<{
    running: number[];
    done: { input: string; output: string }[];
    failed: { input: string; message: string; originalIntact: boolean }[];
  }>({ running: [], done: [], failed: [] });

  /** A plain-language message when the clip couldn't be opened. */
  error = $state<string | null>(null);

  private drag: { segId: string; edge: 'start' | 'end'; base: Timeline } | null = null;

  get timeline(): Timeline {
    return this.preview ?? this.history.present;
  }

  get edited(): boolean {
    return isEdited(this.history);
  }

  /** Quick export at source quality: the source's average rate × the kept video length. */
  get estimateBytes(): number {
    const m = this.media;
    if (!m || !(m.duration_s > 0)) return 0;
    return (m.size_bytes / m.duration_s) * videoEnd(this.timeline);
  }

  /** Starts a fresh edit of `media`: one segment per track, nothing selected, no undo. */
  load(media: EditorMedia): void {
    const audio = media.tracks.filter((t) => !t.empty).map((t) => t.index);
    this.media = media;
    this.history = initHistory(createTimeline(media.duration_s, media.video?.fps ?? DEFAULT_FPS, audio));
    this.endLive();
    this.selection = [];
    this.playhead = 0;
    this.scrollX = 0;
    this.needsFit = true;
    this.peaks = {};
    this.thumbs = [];
    this.shuttle = STOPPED;
    this.proxyFraction = null;
    this.error = null;
  }

  /** Records `next` as an undo step (a no-op edit that returns the same timeline records nothing). */
  edit(next: Timeline): void {
    this.endLive();
    this.history = push(this.history, next);
    this.afterChange();
  }

  // --- keyboard and buttons ----------------------------------------------------------------

  /** Runs one keyboard command (keys.ts) against the timeline, selection, playhead and shuttle. */
  command(c: Command): void {
    const tl = this.timeline;
    const end = videoEnd(tl);
    switch (c) {
      case 'playPause':
        if (this.shuttle.dir !== 0) {
          this.shuttle = STOPPED;
        } else {
          // From the end, play starts over (there's nothing left to play).
          if (this.playhead >= end - 0.5 / tl.fps) this.seek(0);
          this.shuttle = PLAY_1X;
        }
        return;
      case 'J':
      case 'K':
      case 'L':
        this.shuttle = shuttle(this.shuttle, c);
        return;
      case 'frameBack':
      case 'frameFwd':
      case 'secBack':
      case 'secFwd': {
        // A step means "show me that frame", so it stops playback, as in Resolve.
        this.shuttle = STOPPED;
        const n = { frameBack: -1, frameFwd: 1, secBack: -tl.fps, secFwd: tl.fps }[c];
        this.seek(stepFrames(this.playhead, tl.fps, n, end));
        return;
      }
      case 'prevCut':
        this.seek(prevCut(tl, this.playhead));
        return;
      case 'nextCut':
        this.seek(nextCut(tl, this.playhead));
        return;
      case 'home':
        this.seek(0);
        return;
      case 'end':
        this.seek(end);
        return;
      case 'cut':
        this.edit(cutAt(tl, this.playhead));
        return;
      case 'delete':
        // The deleted ids drop out of the selection in afterChange.
        if (this.selection.length > 0) this.edit(deleteSegs(tl, this.selection, this.linked));
        return;
      case 'undo':
      case 'redo':
        this.endLive();
        this.history = c === 'undo' ? undo(this.history) : redo(this.history);
        this.afterChange();
        return;
      case 'toggleSnap':
        this.snapping = !this.snapping;
        return;
      case 'toggleLinked':
        this.linked = !this.linked;
        return;
    }
  }

  // --- selection and playhead --------------------------------------------------------------

  /** Selects a segment, and with Linked on, the pieces on other tracks made by the same cuts. */
  selectAt(segId: string): void {
    this.selection = this.linked ? linkedIds(this.timeline, segId) : [segId];
  }

  clearSelection(): void {
    this.selection = [];
  }

  /** Moves the playhead to `t`, on the frame grid inside [0, videoEnd], and keeps it in view. */
  seek(t: number): void {
    const tl = this.timeline;
    this.playhead = clamp(quantize(t, tl.fps), 0, videoEnd(tl));
    this.reveal();
  }

  /** While playing: pages the view when the playhead runs off either side. */
  follow(): void {
    const w = this.viewWidth;
    if (w <= 0) return;
    const at = this.playhead * this.pxPerSecond;
    const x = at - this.scrollX;
    if (x > w - FOLLOW_MARGIN_PX) this.setScroll(at - FOLLOW_MARGIN_PX);
    else if (x < 0) this.setScroll(at - (w - FOLLOW_MARGIN_PX));
  }

  /** Scrolls so an off-screen playhead lands in the middle of the lanes. */
  private reveal(): void {
    const w = this.viewWidth;
    if (w <= 0) return;
    const x = this.playhead * this.pxPerSecond - this.scrollX;
    if (x < 0 || x > w) this.setScroll(this.playhead * this.pxPerSecond - w / 2);
  }

  // --- zoom and pan ------------------------------------------------------------------------

  /** Zooms by `factor` around `anchorX` (px from the lanes' left edge). */
  zoomAt(factor: number, anchorX: number): void {
    const z = zoomAround(this.pxPerSecond, factor, anchorX, this.scrollX);
    this.pxPerSecond = z.pxPerSecond;
    this.setScroll(z.scrollX);
  }

  /** Scrolls the lanes by `dx` px (positive = later). */
  pan(dx: number): void {
    this.setScroll(this.scrollX + dx);
  }

  /** Never before 0, and never so far that the material leaves the lanes entirely. */
  private setScroll(x: number): void {
    const tl = this.timeline;
    const ends = tl.audio.flatMap((a) => a.segs.map((s) => s.at + segLen(s)));
    const contentPx = Math.max(videoEnd(tl), ...ends) * this.pxPerSecond;
    const max = this.viewWidth > 0 ? Math.max(0, contentPx + PAN_MARGIN_PX - this.viewWidth) : Infinity;
    this.scrollX = clamp(x, 0, max);
  }

  // --- edge drags --------------------------------------------------------------------------

  beginEdgeDrag(segId: string, edge: 'start' | 'end'): void {
    this.endLive();
    this.drag = { segId, edge, base: this.history.present };
  }

  /** Moves the dragged edge to `t` (snapped when Snap is on); shown live, not yet an undo step. */
  dragEdgeTo(t: number): void {
    const d = this.drag;
    if (!d) return;
    let to = t;
    this.snapAt = null;
    if (this.snapping) {
      const candidates = snapCandidates(d.base, this.playhead, { segId: d.segId, edge: d.edge }, this.linked);
      const snapped = snap(t, candidates, SNAP_PX / this.pxPerSecond);
      if (snapped !== t) {
        to = snapped;
        this.snapAt = snapped;
      }
    }
    this.preview = moveEdge(d.base, d.segId, d.edge, to, this.linked);
  }

  /** Releases the drag: the whole drag becomes one undo step (none if nothing changed). */
  endEdgeDrag(): void {
    if (!this.drag) return;
    this.edit(this.preview ?? this.drag.base);
  }

  // --- track volume and mute ---------------------------------------------------------------

  /** Sets a track's gain. While the slider moves (`commit` false) it's live only; release records it. */
  setGain(index: number, db: number, commit: boolean): void {
    if (commit) {
      this.edit(setGain(this.history.present, index, db));
    } else {
      this.drag = null;
      this.preview = setGain(this.preview ?? this.history.present, index, db);
    }
  }

  toggleMute(index: number): void {
    const track = this.history.present.audio.find((a) => a.index === index);
    if (track) this.edit(setMuted(this.history.present, index, !track.muted));
  }

  // --- internals ---------------------------------------------------------------------------

  /** Drops any live drag or slide state. */
  private endLive(): void {
    this.drag = null;
    this.preview = null;
    this.snapAt = null;
  }

  /** After the timeline changes: the playhead stays inside the video, the selection on real segs. */
  private afterChange(): void {
    const tl = this.timeline;
    const end = videoEnd(tl);
    if (this.playhead > end) this.playhead = end;
    if (this.selection.length > 0) {
      const ids = new Set([...tl.video, ...tl.audio.flatMap((a) => a.segs)].map((s) => s.id));
      const kept = this.selection.filter((id) => ids.has(id));
      if (kept.length !== this.selection.length) this.selection = kept;
    }
  }
}
