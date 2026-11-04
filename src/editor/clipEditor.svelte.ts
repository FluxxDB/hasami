// One clip's edit in the Quick Trim editor: the timeline and its undo history, the selection, the
// playhead and shuttle, the view of the lanes (zoom and scroll), edge drags and volume. Edits go
// through `edit()` so every one is an undo step. The timeline itself lives in `history.present`,
// except while an edge drag or a volume slider is moving: that live state is `preview`, and it
// becomes one undo step when the drag or the slider is released.
//
// `EditorStore` (store.svelte.ts) builds on it with the trim queue around the clip: opening clips,
// Prev / Next, exports and Discard. It decides when the clip is `locked`.

import type { EditorMedia, Peaks, Thumb } from './api';
import { estimateExportBytes } from './estimate';
import { SNAP_PX, zoomAround } from './geometry';
import { initHistory, isEdited, push, redo, undo, type History } from './history';
import type { Command } from './keys';
import { STOPPED, nextCut, prevCut, shuttle, stepFrames, type Shuttle } from './playbackModel';
import {
  createTimeline,
  cutAt,
  deleteJoin,
  deleteSegs,
  hasAudibleAudio,
  linkedIds,
  MASTER_STEP_DB,
  moveEdge,
  quantize,
  segLen,
  setGain,
  setMaster,
  setMuted,
  snap,
  snapCandidates,
  videoEnd,
  type Crop,
  type Timeline,
} from './timeline';

/** Keyboard commands that change the timeline, refused while the clip is locked. */
const EDITS = new Set<Command>(['cut', 'delete', 'undo', 'redo']);
/** The frame rate of audio-only files, which have none of their own. */
export const DEFAULT_FPS = 30;
const PLAY_1X: Shuttle = { dir: 1, speed: 1 };
/** Room kept after the end of the material when panning, so the last edge stays grabbable. */
const PAN_MARGIN_PX = 24;
/** How near the lanes' right edge the playhead may get while playing before the view pages. */
const FOLLOW_MARGIN_PX = 24;

const clamp = (x: number, lo: number, hi: number): number => Math.min(Math.max(x, lo), hi);

export class ClipEditor {
  // Raw (not deeply reactive): history snapshots are compared by identity and never mutated.
  media = $state.raw<EditorMedia | null>(null);
  history = $state.raw<History<Timeline>>(initHistory(createTimeline(0, DEFAULT_FPS, [])));
  /** The live timeline during an edge drag or a volume slide; null otherwise. */
  preview = $state.raw<Timeline | null>(null);
  selection = $state.raw<string[]>([]);
  /** Written by `seek` (and by the playback engine while it plays). */
  playhead = $state(0);
  /**
   * Bumped on every seek, so the window forwards each one to the playback engine, even one that
   * lands on the time the engine last reported (the engine's own reports don't bump it).
   */
  seekSeq = $state(0);

  pxPerSecond = $state(20);
  scrollX = $state(0);
  /** Width of the lanes in px, measured by the window (0 until laid out). */
  viewWidth = $state(0);
  /** Set by `load`; the window fits the clip to the lanes once it knows their width. */
  needsFit = $state(false);

  linked = $state(true);
  snapping = $state(true);
  showEmpty = $state(false);
  /**
   * Dim: everything you hear 11 dB quieter while editing. Never exported and not an undo step;
   * it stays on across clips until turned off (spec 5.4).
   */
  dim = $state(false);
  /**
   * Keep border: exports (and the preview) keep the black border found on open instead of cropping
   * it off (spec 5.7). Off for each clip as it opens; not an undo step.
   */
  keepBorder = $state(false);

  peaks = $state.raw<Record<number, Peaks>>({});
  thumbs = $state.raw<Thumb[]>([]);

  shuttle = $state.raw<Shuttle>(STOPPED);
  /** Where the dragged edge snapped to (drawn as a 1px line); null when it isn't snapped. */
  snapAt = $state<number | null>(null);
  /** Progress (0..1) of the H.264 preview proxy while it's being made; null otherwise. */
  proxyFraction = $state<number | null>(null);

  /** A plain-language message when the clip couldn't be opened. */
  error = $state<string | null>(null);
  /** A plain-language message when an export couldn't start, or Next couldn't look (status bar). */
  notice = $state<string | null>(null);
  private drag: { segId: string; edge: 'start' | 'end'; base: Timeline } | null = null;

  get timeline(): Timeline {
    return this.preview ?? this.history.present;
  }

  get edited(): boolean {
    return isEdited(this.history);
  }

  /** The source's audio tracks with nothing in them: not on the timeline, listed under "+N empty tracks". */
  get emptyTracks(): EditorMedia['tracks'] {
    return this.media?.tracks.filter((t) => t.empty) ?? [];
  }

  get canExportAudio(): boolean {
    return this.media !== null && hasAudibleAudio(this.timeline);
  }

  /**
   * The clip takes no edits (it still plays). Never here; the store locks a clip whose Quick /
   * Save raw export is dealing with its original.
   */
  get locked(): boolean {
    return false;
  }

  /**
   * The black border found around the open clip's picture (spec 5.7), as the rectangle inside it;
   * null when there's none. A rectangle with no area is no border (it would crop to nothing).
   */
  get border(): Crop | null {
    const m = this.media;
    const b = m?.video ? m.border : null;
    return b && b.w > 0 && b.h > 0 ? b : null;
  }

  /** The picture video exports keep, and the viewer shows: the border's inside, unless Keep border is on. */
  get crop(): Crop | null {
    const border = this.border;
    return border && !this.keepBorder ? border : null;
  }

  /** What a Quick export of the edit will weigh, for the status bar (estimate.ts). */
  get estimateBytes(): number {
    return estimateExportBytes(this.media, this.timeline);
  }

  /** Starts a fresh edit of `media`: one segment per track, nothing selected, no undo. */
  load(media: EditorMedia): void {
    this.clear();
    const audio = media.tracks.filter((t) => !t.empty).map((t) => t.index);
    this.media = media;
    this.history = initHistory(createTimeline(media.duration_s, media.video?.fps ?? DEFAULT_FPS, audio));
    this.keepBorder = false;
    this.scrollX = 0;
    this.needsFit = true;
  }

  /** Lets go of the clip: an empty timeline, nothing selected, nothing playing, no messages. */
  protected clear(): void {
    this.media = null;
    this.history = initHistory(createTimeline(0, DEFAULT_FPS, []));
    this.endLive();
    this.selection = [];
    this.movePlayhead(0);
    this.peaks = {};
    this.thumbs = [];
    this.shuttle = STOPPED;
    this.proxyFraction = null;
    this.error = null;
    this.notice = null;
  }

  /** Records `next` as an undo step (a no-op edit that returns the same timeline records nothing). */
  edit(next: Timeline): void {
    this.endLive();
    if (this.locked) return;
    this.history = push(this.history, next);
    this.afterChange();
  }

  // --- keyboard and buttons ----------------------------------------------------------------

  /** Runs one keyboard command (keys.ts) against the timeline, selection, playhead and shuttle. */
  command(c: Command): void {
    // A locked clip's original is about to move: it takes no more edits, but still plays.
    if (this.locked && EDITS.has(c)) return;
    // An edit made mid-drag (or mid-slide) lands on top of it as its own undo step.
    if (c === 'cut' || c === 'delete') this.commitLive();
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
      case 'cut': {
        // With a selected clip under the playhead, only the selection is cut; otherwise every
        // track. The left pieces keep their ids, so the selection stays on them.
        const t = this.playhead;
        const sel = new Set(this.selection);
        const underPlayhead = [...tl.video, ...tl.audio.flatMap((a) => a.segs)].some(
          (s) => sel.has(s.id) && s.at < t && t < s.at + segLen(s),
        );
        this.edit(underPlayhead ? cutAt(tl, t, this.selection) : cutAt(tl, t));
        return;
      }
      case 'delete':
        // The deleted ids drop out of the selection in afterChange.
        if (this.selection.length > 0) {
          // The playhead lands at the join; an audio-only delete (no join) leaves it be.
          const join = deleteJoin(tl, this.selection, this.linked);
          this.edit(deleteSegs(tl, this.selection, this.linked));
          if (join !== null && this.timeline !== tl) this.seek(join);
        }
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
      case 'selectAtPlayhead': {
        // The video piece under the playhead (on a cut, the one that starts there), picked as a
        // click would pick it. With no video there, the selection stays as it was.
        const t = this.playhead;
        const seg = tl.video.find((s) => t >= s.at && t < s.at + segLen(s));
        if (seg) this.selectAt(seg.id);
        return;
      }
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
    this.movePlayhead(clamp(quantize(t, tl.fps), 0, videoEnd(tl)));
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
    if (this.locked) return;
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
    } else if (!this.locked) {
      this.drag = null;
      this.preview = setGain(this.preview ?? this.history.present, index, db);
    }
  }

  /** Sets the master volume. While the slider moves (`commit` false) it's live only; release records it. */
  setMaster(db: number, commit: boolean): void {
    if (commit) {
      this.edit(setMaster(this.history.present, db));
    } else if (!this.locked) {
      this.drag = null;
      this.preview = setMaster(this.preview ?? this.history.present, db);
    }
  }

  /** The master − / + buttons: one 3 dB step, one undo entry (none once clamped). */
  nudgeMaster(dir: 1 | -1): void {
    this.edit(setMaster(this.history.present, this.history.present.masterDb + dir * MASTER_STEP_DB));
  }

  toggleMute(index: number): void {
    const track = this.history.present.audio.find((a) => a.index === index);
    if (track) this.edit(setMuted(this.history.present, index, !track.muted));
  }

  // --- internals ---------------------------------------------------------------------------

  private movePlayhead(t: number): void {
    this.playhead = t;
    this.seekSeq += 1;
  }

  /** Records a live drag or slide as its undo step now. */
  protected commitLive(): void {
    if (this.preview) this.edit(this.preview);
    else this.endLive();
  }

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
    if (this.playhead > end) this.movePlayhead(end);
    if (this.selection.length > 0) {
      const ids = new Set([...tl.video, ...tl.audio.flatMap((a) => a.segs)].map((s) => s.id));
      const kept = this.selection.filter((id) => ids.has(id));
      if (kept.length !== this.selection.length) this.selection = kept;
    }
  }
}
