// The Quick Trim editor's state. Components render from it; edits go through `edit()` so every
// one is an undo step. The timeline itself lives in `history.present`, except while an edge drag
// or a volume slider is moving: that live state is `preview`, and it becomes one undo step when
// the drag or the slider is released.
//
// It also runs the trim queue: which file is open, the exports handed to the Rust job queue and
// how they ended, and the end-of-queue summary. Leaving a clip (export, Skip, Discard) closes its
// editor session on the Rust side.

import { api, type EditorEvent, type EditorMedia, type Peaks, type Thumb } from './api';
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
  hasAudibleAudio,
  toExportRequest,
  videoEnd,
  type ExportRequest,
  type Timeline,
} from './timeline';

export type ExportMode = ExportRequest['mode'];

type LogEntry =
  | { action: ExportMode; id: number; input: string }
  | { action: 'skip'; input: string }
  /**
   * A discard of a clip with exports still to finish (an Export audio still queued reads the
   * file) is carried out by the Rust side once they're done; `Discard*` events report it.
   */
  | { action: 'discard'; input: string; state: 'waiting' | 'done' | 'kept'; message?: string };
/** What the unsaved-edits dialog is asking about. */
export type ConfirmKind = 'close' | 'skip' | 'discard';

/** One line of the end-of-queue summary: something done to a file, and how it ended. */
export interface Result {
  input: string;
  action: ExportMode | 'skip' | 'discard';
  /** `none` for a skip or a done discard; `running` while a discard waits; `failed` when it kept the clip. */
  state: 'running' | 'done' | 'failed' | 'none';
  fraction: number;
  output?: string;
  bytes?: number;
  message?: string;
  originalIntact?: boolean;
}

// Shown when a command fails without a message of its own.
const OPEN_FAILED = "Couldn't open this clip.";
const EXPORT_FAILED = "Couldn't start the export.";
const DISCARD_FAILED = "Couldn't send this clip to the Recycle Bin.";

/** The Rust commands answer failures with a plain-language string. */
const plain = (err: unknown, fallback: string): string => (typeof err === 'string' && err ? err : fallback);

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
    /** Job ids handed to the queue and not yet finished. */
    running: number[];
    done: { id: number; input: string; output: string; bytes: number }[];
    failed: { id: number; input: string; message: string; originalIntact: boolean }[];
    /** Latest progress (0..1) of each running export. */
    progress: Record<number, number>;
  }>({ running: [], done: [], failed: [], progress: {} });
  /** What was done to each file, in order, for the summary. */
  log = $state<LogEntry[]>([]);

  /** A plain-language message when the clip couldn't be opened. */
  error = $state<string | null>(null);
  /** A plain-language message when an export couldn't start or a discard failed (status bar). */
  notice = $state<string | null>(null);
  /** The unsaved-edits question being asked, if any; `answer` replies. */
  confirm = $state<ConfirmKind | null>(null);
  /** An export, Skip or Discard is under way; the buttons wait for it. */
  busy = $state(false);

  /** Called after each clip loads (the browser mock applies its demo edits here). */
  afterLoad: ((store: EditorStore) => void) | null = null;

  private drag: { segId: string; edge: 'start' | 'end'; base: Timeline } | null = null;
  private reply: ((ok: boolean) => void) | null = null;
  /** Exports already finished, in case their last event beats `editor_export`'s answer. */
  private finished = new Set<number>();
  /** Bumped by each open, so an answer for a clip no longer wanted is dropped. */
  private openSeq = 0;

  get timeline(): Timeline {
    return this.preview ?? this.history.present;
  }

  get edited(): boolean {
    return isEdited(this.history);
  }

  /** The file the queue is on (open, opening, or failed to open); null at the summary. */
  get currentPath(): string | null {
    return this.queue.files[this.queue.index] ?? null;
  }

  /** More than one file: the top bar shows "2 OF 3", Skip and Discard. */
  get queueMode(): boolean {
    return this.queue.files.length > 1;
  }

  /** Every file has been dealt with. */
  get showSummary(): boolean {
    return this.queue.files.length > 0 && this.queue.index >= this.queue.files.length;
  }

  get canExportAudio(): boolean {
    return this.media !== null && hasAudibleAudio(this.timeline);
  }

  /**
   * The window can close by itself: every file is dealt with and no export failed. A queue waits
   * for its exports, so the summary can report them; a single clip closes as soon as its export
   * is handed over (spec 5.7), and a late failure shows in the progress window instead.
   */
  get shouldClose(): boolean {
    const e = this.exports;
    const discarding = this.log.some((l) => l.action === 'discard' && l.state !== 'done');
    return this.showSummary && e.failed.length === 0 && !discarding && (e.running.length === 0 || !this.queueMode);
  }

  /** The summary's lines, in the order things were done. */
  get results(): Result[] {
    const e = this.exports;
    return this.log.map((entry): Result => {
      if (entry.action === 'skip') return { input: entry.input, action: 'skip', state: 'none', fraction: 0 };
      if (entry.action === 'discard') {
        const state = entry.state === 'done' ? 'none' : entry.state === 'kept' ? 'failed' : 'running';
        return { input: entry.input, action: 'discard', state, fraction: 0, message: entry.message };
      }
      const { id, input, action } = entry;
      const done = e.done.find((d) => d.id === id);
      if (done) return { input, action, state: 'done', fraction: 1, output: done.output, bytes: done.bytes };
      const failed = e.failed.find((f) => f.id === id);
      if (failed) {
        const { message, originalIntact } = failed;
        return { input, action, state: 'failed', fraction: 0, message, originalIntact };
      }
      return { input, action, state: 'running', fraction: e.progress[id] ?? 0 };
    });
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
    this.movePlayhead(0);
    this.scrollX = 0;
    this.needsFit = true;
    this.peaks = {};
    this.thumbs = [];
    this.shuttle = STOPPED;
    this.proxyFraction = null;
    this.error = null;
    this.notice = null;
  }

  // --- queue -------------------------------------------------------------------------------

  /**
   * Adds files to the end of the queue, once per path among those not dealt with yet (a file
   * already exported, skipped, kept or failed can be trimmed again; one waiting to be recycled,
   * or to be replaced or moved by a Quick / Save raw export still to finish, can't). At the
   * summary, the first opens.
   */
  addFiles(paths: string[]): void {
    const resume = this.showSummary;
    // A clip waiting to be recycled or moved is as good as gone: don't open it again.
    const running = new Set(this.exports.running);
    const leaving = this.log.flatMap((l) =>
      (l.action === 'discard' && l.state === 'waiting') ||
      ((l.action === 'quick' || l.action === 'save_raw') && running.has(l.id))
        ? [l.input]
        : [],
    );
    const pending = new Set([...this.queue.files.slice(this.queue.index), ...leaving]);
    for (const p of paths) {
      if (pending.has(p)) continue;
      pending.add(p);
      this.queue.files.push(p);
    }
    const next = this.currentPath;
    if (resume && next) void this.openFile(next);
  }

  /** Opens `path` and loads it; thumbnails and waveforms fill in as they arrive (spec 6). */
  async openFile(path: string): Promise<void> {
    const seq = ++this.openSeq;
    this.error = null;
    let media: EditorMedia;
    try {
      media = await api.open(path);
    } catch (err) {
      if (seq === this.openSeq) this.error = plain(err, OPEN_FAILED);
      return;
    }
    if (seq !== this.openSeq || this.currentPath !== path) {
      void this.closeSession(media.session); // moved on while it opened
      return;
    }
    this.load(media);
    this.afterLoad?.(this);

    // A late answer for a clip that is no longer open is dropped.
    const session = media.session;
    const current = () => this.media?.session === session;
    api
      .thumbs(session)
      .then((thumbs) => current() && (this.thumbs = thumbs))
      .catch((err) => console.error('editor_thumbs failed', err));
    for (const t of media.tracks.filter((t) => !t.empty)) {
      api
        .peaks(session, t.index)
        .then((p) => current() && (this.peaks = { ...this.peaks, [t.index]: p }))
        .catch((err) => console.error('editor_peaks failed', err));
    }
  }

  /** Leaves the current clip and opens the next queued file; with none left, the summary shows. */
  async advance(): Promise<void> {
    void this.leave();
    await this.next();
  }

  /** Hands the edit to the job queue. A video export then moves on; Export audio stays. */
  async exportClip(mode: ExportMode): Promise<void> {
    const m = this.media;
    if (!m || this.busy || (mode === 'audio' && !this.canExportAudio)) return;
    this.commitLive();
    const fit20 = mode !== 'audio' && this.fit20;
    const request = toExportRequest(this.history.present, m.session, m.path, mode, fit20);
    this.busy = true;
    this.notice = null;
    try {
      // Waited for: until the queue has the job, closing the window could lose it.
      let id: number;
      try {
        id = await api.exportClip(request);
      } catch (err) {
        this.notice = plain(err, EXPORT_FAILED);
        return;
      }
      this.log.push({ action: mode, id, input: m.path });
      this.track(id);
      if (mode !== 'audio') await this.advance();
    } finally {
      this.busy = false;
    }
  }

  /** Moves on and leaves the file as it is; an edited clip asks first. */
  async skip(): Promise<void> {
    const path = this.currentPath;
    if (!path || this.busy) return;
    this.busy = true;
    try {
      if (this.edited && !(await this.ask('skip'))) return;
      this.log.push({ action: 'skip', input: path });
      await this.advance();
    } finally {
      this.busy = false;
    }
  }

  /** Sends the file to the Recycle Bin and moves on; an edited clip asks first. */
  async discard(): Promise<void> {
    const path = this.currentPath;
    if (!path || this.busy) return;
    this.busy = true;
    try {
      if (this.edited && !(await this.ask('discard'))) return;
      // The session's ffmpeg runs (thumbnails, waveforms) let go of the file before it's recycled.
      await this.leave();
      // An export of this clip still to finish (an Export audio) needs the file: the Rust side
      // recycles it after that, or keeps it if the export fails.
      const after = this.runningExportsOf(path);
      // Logged before asking: Rust may report the outcome (Discard*) before its reply arrives.
      const at = this.log.length;
      this.log.push({ action: 'discard', input: path, state: after.length > 0 ? 'waiting' : 'done' });
      try {
        await api.discard(path, after);
      } catch (err) {
        this.log.splice(at, 1);
        await this.openFile(path);
        this.notice = plain(err, DISCARD_FAILED);
        return;
      }
      await this.next();
    } finally {
      this.busy = false;
    }
  }

  /** Asks the unsaved-edits question; resolves true to go ahead, false to keep editing. */
  ask(kind: ConfirmKind): Promise<boolean> {
    this.reply?.(false);
    this.confirm = kind;
    return new Promise((resolve) => (this.reply = resolve));
  }

  answer(ok: boolean): void {
    const reply = this.reply;
    this.reply = null;
    this.confirm = null;
    reply?.(ok);
  }

  /** Applies an event from the Rust side. */
  applyEvent(e: EditorEvent): void {
    const x = this.exports;
    switch (e.type) {
      case 'Files':
        this.addFiles(e.paths);
        return;
      case 'ExportStarted':
        this.track(e.id);
        return;
      case 'ExportProgress':
        if (x.running.includes(e.id)) x.progress[e.id] = e.fraction;
        return;
      case 'ExportDone':
        this.finish(e.id);
        x.done.push({ id: e.id, input: e.input, output: e.output, bytes: e.bytes });
        return;
      case 'ExportFailed':
        this.finish(e.id);
        x.failed.push({ id: e.id, input: e.input, message: e.message, originalIntact: e.original_intact });
        return;
      case 'ProxyProgress':
        if (e.session === this.media?.session && this.proxyFraction !== null) this.proxyFraction = e.fraction;
        return;
      case 'DiscardDone':
      case 'DiscardKept': {
        const entry = this.log.find((l) => l.action === 'discard' && l.input === e.path && l.state === 'waiting');
        if (entry?.action !== 'discard') return;
        entry.state = e.type === 'DiscardDone' ? 'done' : 'kept';
        if (e.type === 'DiscardKept') entry.message = e.message;
        return;
      }
    }
  }

  /** Moves to the next queued file (the current one has been left); none left: the summary. */
  private async next(): Promise<void> {
    this.queue.index += 1;
    const path = this.currentPath;
    if (path) await this.openFile(path);
  }

  /** Follows export `id` until it finishes (a no-op for one already followed or finished). */
  private track(id: number): void {
    if (!this.finished.has(id) && !this.exports.running.includes(id)) this.exports.running.push(id);
  }

  private finish(id: number): void {
    this.finished.add(id);
    const x = this.exports;
    x.running = x.running.filter((r) => r !== id);
    delete x.progress[id];
  }

  /** The job ids of exports of `path` still queued or running. */
  private runningExportsOf(path: string): number[] {
    const running = new Set(this.exports.running);
    return this.log.flatMap((l) => ('id' in l && l.input === path && running.has(l.id) ? [l.id] : []));
  }

  /**
   * Lets go of the open clip: nothing of it stays on screen, and its session closes (resolves
   * once the Rust side has stopped its work; only Discard needs to wait for that).
   */
  private leave(): Promise<void> {
    this.openSeq += 1; // an open still in flight is no longer wanted
    const closed = this.media ? this.closeSession(this.media.session) : Promise.resolve();
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
    return closed;
  }

  private closeSession(session: number): Promise<void> {
    return api.close(session).catch((err) => console.error('editor_close failed', err));
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

  private movePlayhead(t: number): void {
    this.playhead = t;
    this.seekSeq += 1;
  }

  /** Records a live drag or slide as its undo step now. */
  private commitLive(): void {
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
