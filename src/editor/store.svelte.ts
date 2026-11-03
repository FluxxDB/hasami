// The Quick Trim editor's state. Components render from it; edits go through `edit()` so every
// one is an undo step. The timeline itself lives in `history.present`, except while an edge drag
// or a volume slider is moving: that live state is `preview`, and it becomes one undo step when
// the drag or the slider is released.
//
// It also runs the trim queue (spec 5.7, 5.8): which file is open, the exports handed to the Rust
// job queue and how they're going, and the results list. The window never closes by itself: a
// clip stays open while it exports (a Quick / Save raw export locks it), and Prev / Next move on.
// The queue is ordered by date, oldest first, and opens on the oldest: Next goes to the next newer
// clip and Prev to the next older one, through the queued files and then on into the folder past
// either end. Leaving a clip (Prev, Next, Discard) closes its editor session on the Rust side.

import { api, type EditorEvent, type EditorMedia, type Peaks, type Thumb } from './api';
import { SNAP_PX, zoomAround } from './geometry';
import { initHistory, isEdited, push, redo, undo, type History } from './history';
import type { Command } from './keys';
import { STOPPED, nextCut, prevCut, shuttle, stepFrames, type Shuttle } from './playbackModel';
import {
  createTimeline,
  cutAt,
  deleteJoin,
  deleteSegs,
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
  hasAudibleAudio,
  toExportRequest,
  videoEnd,
  type ExportRequest,
  type Timeline,
} from './timeline';

export type ExportMode = ExportRequest['mode'];

type LogEntry =
  /** `visit` is the stay on the clip it was started from (see `EditorStore.visit`). */
  | { action: ExportMode; id: number; input: string; visit: number }
  /**
   * A discard of a clip with exports still to finish (an Export audio still queued reads the
   * file) is carried out by the Rust side once they're done; `Discard*` events report it.
   */
  | { action: 'discard'; input: string; state: 'waiting' | 'done' | 'kept'; message?: string };
/** What the unsaved-edits dialog is asking about: closing, moving on (Prev / Next), or Discard. */
export type ConfirmKind = 'close' | 'move' | 'discard';
/** Next video goes to the next newer clip, Prev to the next older one. */
export type Direction = 'next' | 'prev';

/** One line of the results list: something done to a file, and how it stands. */
export interface Result {
  input: string;
  action: ExportMode | 'discard';
  /**
   * An export is `queued` until the job queue starts it. A discard is `running` while it waits
   * for an export, `none` once done, and `failed` when it kept the clip.
   */
  state: 'queued' | 'running' | 'done' | 'failed' | 'none';
  fraction: number;
  output?: string;
  bytes?: number;
  /** Said about a saved export, e.g. that its 20 MB copy couldn't be made. */
  note?: string;
  message?: string;
  originalIntact?: boolean;
}

/**
 * The file that's open (or opening, or failed to open). `at` is its place in the queue, null for
 * one from the folder; `side` says which end of the queue such a folder clip lies past.
 */
export interface Current {
  path: string;
  at: number | null;
  side?: 'older' | 'newer';
}

// Shown when a command fails without a message of its own.
const OPEN_FAILED = "Couldn't open this clip.";
const EXPORT_FAILED = "Couldn't start the export.";
const DISCARD_FAILED = "Couldn't send this clip to the Recycle Bin.";
const LOOK_FAILED: Record<Direction, string> = {
  next: "Couldn't look for the next video in this folder.",
  prev: "Couldn't look for the previous video in this folder.",
};

/** The Rust commands answer failures with a plain-language string. */
const plain = (err: unknown, fallback: string): string => (typeof err === 'string' && err ? err : fallback);

/** Keyboard commands that change the timeline, refused while the clip is locked. */
const EDITS = new Set<Command>(['cut', 'delete', 'undo', 'redo']);
/** How many clips on their way out the folder walk passes over before it gives up. */
const MAX_PASSED = 50;

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
  /** The 20 MB copy toggle: video exports also save a ≤ 20 MB copy to Videos. */
  fit20 = $state(false);
  showEmpty = $state(false);
  /**
   * Dim: everything you hear 11 dB quieter while editing. Never exported and not an undo step;
   * it stays on across clips until turned off (spec 5.4).
   */
  dim = $state(false);

  peaks = $state.raw<Record<number, Peaks>>({});
  thumbs = $state.raw<Thumb[]>([]);

  shuttle = $state.raw<Shuttle>(STOPPED);
  /** Where the dragged edge snapped to (drawn as a 1px line); null when it isn't snapped. */
  snapAt = $state<number | null>(null);
  /** Progress (0..1) of the H.264 preview proxy while it's being made; null otherwise. */
  proxyFraction = $state<number | null>(null);

  /**
   * The files Hasami was asked to trim, oldest first. `next` is the first not opened yet; the
   * files from there on are re-sorted when more arrive.
   */
  queue = $state<{ files: string[]; next: number }>({ files: [], next: 0 });
  /** The file on screen; null before the first and after the last (nothing left to trim). */
  current = $state<Current | null>(null);
  /** Counts the stays on a clip: each open of one is a new visit, and its exports belong to it. */
  visit = $state(0);
  /**
   * Where Prev / Next go through the folder from the open clip, checked ahead of time so their
   * disabled states are honest: undefined while checking (or when the queue has a step that way
   * anyway, or when the check failed: see `aheadUnknown`), null for nothing that way.
   */
  ahead = $state.raw<Record<Direction, Current | null | undefined>>({ next: undefined, prev: undefined });
  /**
   * The look-ahead couldn't read the folder. The button stays enabled: pressing it looks again, and
   * says why if it still can't (a failed check must not pass for "nothing that way").
   */
  aheadUnknown = $state.raw<Record<Direction, boolean>>({ next: false, prev: false });
  exports = $state<{
    /** Job ids handed to the queue and not yet finished. */
    running: number[];
    done: { id: number; input: string; output: string; bytes: number; note?: string }[];
    failed: { id: number; input: string; message: string; originalIntact: boolean }[];
    /** Latest progress (0..1) of each export the queue has started (absent: still queued). */
    progress: Record<number, number>;
  }>({ running: [], done: [], failed: [], progress: {} });
  /** What was done to each file, in order, for the results list. */
  log = $state<LogEntry[]>([]);
  /** The export being handed to the job queue (until `editor_export` answers); null otherwise. */
  handing = $state<ExportMode | null>(null);

  /** A plain-language message when the clip couldn't be opened. */
  error = $state<string | null>(null);
  /** A plain-language message when an export couldn't start, a discard failed, or Next couldn't look (status bar). */
  notice = $state<string | null>(null);
  /** The unsaved-edits question being asked, if any; `answer` replies. */
  confirm = $state<ConfirmKind | null>(null);
  /** An export hand-over, Next or Discard is under way; the buttons wait for it. */
  busy = $state(false);

  /** Called after each clip loads (the browser mock applies its demo edits here). */
  afterLoad: ((store: EditorStore) => void) | null = null;

  private drag: { segId: string; edge: 'start' | 'end'; base: Timeline } | null = null;
  private reply: ((ok: boolean) => void) | null = null;
  /** Exports already finished, in case their last event beats `editor_export`'s answer. */
  private finished = new Set<number>();
  /** The timeline each export was made from, so Next and close know which edits went out. */
  private exported = new Map<number, Timeline>();
  /**
   * Each opened file's modified time. Next finds the folder's next video from it, even once the
   * file has been recycled or moved.
   */
  private modified = new Map<string, number>();
  /** Bumped by each open, so an answer for a clip no longer wanted is dropped. */
  private openSeq = 0;
  /** Bumped by each look-ahead, so only the latest one's answer lands. */
  private lookSeq = 0;
  /**
   * Files recycled or moved by a finished Quick / Save raw export or discard: Prev / Next pass over
   * them in the queue. A new trim of one (it was restored) takes it off.
   */
  private gone = $state.raw<ReadonlySet<string>>(new Set());
  /** `Files` batches merge one at a time (each waits for its ordering). */
  private merging: Promise<void> = Promise.resolve();

  get timeline(): Timeline {
    return this.preview ?? this.history.present;
  }

  get edited(): boolean {
    return isEdited(this.history);
  }

  /** The file that's open, opening, or failed to open; null when there's none. */
  get currentPath(): string | null {
    return this.current?.path ?? null;
  }

  /** More than one file queued: the top bar shows "2 OF 3" while on one of them. */
  get queueMode(): boolean {
    return this.queue.files.length > 1;
  }

  /** Everything has been moved on from (e.g. the last video was discarded): nothing is open. */
  get ended(): boolean {
    return this.visit > 0 && this.current === null;
  }

  /** Next video has somewhere to go: a newer queued file, or a newer video in the folder. */
  get canNext(): boolean {
    return this.canGo('next');
  }

  /** Prev has somewhere to go: an older queued file, or an older video in the folder. */
  get canPrev(): boolean {
    return this.canGo('prev');
  }

  /** Where Next goes, for its tooltip: undefined while that's being checked. */
  get nextTarget(): Current | null | undefined {
    return this.target('next');
  }

  /** Where Prev goes, for its tooltip: undefined while that's being checked. */
  get prevTarget(): Current | null | undefined {
    return this.target('prev');
  }

  get canExportAudio(): boolean {
    return this.media !== null && hasAudibleAudio(this.timeline);
  }

  /** The open clip's latest Quick / Save raw export, with how it stands; null when there's none. */
  get videoExport(): Result | null {
    if (this.handing === 'quick' || this.handing === 'save_raw') return this.handover(this.handing);
    return this.latest((a) => a !== 'audio');
  }

  /** The open clip's latest Export audio, with how it stands; null when there's none. */
  get audioExport(): Result | null {
    if (this.handing === 'audio') return this.handover('audio');
    return this.latest((a) => a === 'audio');
  }

  /**
   * A Quick / Save raw export of the open clip is on its way or done, so its original is about
   * to move (or has): no more edits. A failed one unlocks the clip to try again.
   */
  get locked(): boolean {
    const v = this.videoExport;
    return v !== null && v.state !== 'failed';
  }

  /** The open clip has edits that no export (on its way or done) was made from. */
  get unsaved(): boolean {
    if (!this.edited) return false;
    const present = this.history.present;
    return !this.log.some(
      (l) => 'id' in l && l.visit === this.visit && this.exported.get(l.id) === present && !this.failedJob(l.id),
    );
  }

  /** The results list's lines, in the order things were done. */
  get results(): Result[] {
    return this.log.map((entry) => this.resultOf(entry));
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
   * Merges files into the part of the queue not opened yet and sorts that part by date, oldest
   * first; the files already opened keep their places. Each path goes in once among those not
   * dealt with yet (a file already exported, moved on from, kept or failed can be trimmed again;
   * one waiting to be recycled, or to be replaced or moved by a Quick / Save raw export still to
   * finish, can't). With nothing open (the first batch, or after the last video), the first of the
   * part not opened yet opens: for the first batch, the oldest file.
   */
  addFiles(paths: string[]): Promise<void> {
    const run = this.merging.then(() => this.merge(paths));
    this.merging = run.catch(() => {});
    return run;
  }

  private async merge(paths: string[]): Promise<void> {
    const q = this.queue;
    const ahead = q.files.slice(q.next);
    const pending = new Set([...ahead, ...(this.current ? [this.current.path] : []), ...this.leaving()]);
    const fresh: string[] = [];
    for (const p of paths) {
      if (pending.has(p)) continue;
      pending.add(p);
      fresh.push(p);
    }
    if (fresh.length > 0) {
      // Asked for again, so it's back (restored from the Recycle Bin, say).
      if (fresh.some((p) => this.gone.has(p))) this.gone = new Set([...this.gone].filter((p) => !fresh.includes(p)));
      let order: string[];
      try {
        order = await api.orderFiles([...ahead, ...fresh]);
      } catch (err) {
        console.error('editor_order_files failed', err);
        order = [...ahead, ...fresh];
      }
      // Next may have opened some of them while they were being ordered: those keep their places.
      // Anything the ordering left out goes last, as it came.
      const wanted = new Set([...q.files.slice(q.next), ...fresh.filter((p) => p !== this.current?.path)]);
      const sorted = [...new Set([...order, ...wanted])].filter((p) => wanted.has(p));
      q.files = [...q.files.slice(0, q.next), ...sorted];
    }
    if (this.current === null && q.next < q.files.length) await this.show({ path: q.files[q.next], at: q.next });
  }

  /** Hands the edit to the job queue; the clip stays open (a Quick / Save raw export locks it). */
  async exportClip(mode: ExportMode): Promise<void> {
    const m = this.media;
    if (!m || this.busy || this.locked) return;
    if (mode === 'audio' && (!this.canExportAudio || this.inFlight(this.audioExport))) return;
    this.commitLive();
    const fit20 = mode !== 'audio' && this.fit20;
    const timeline = this.history.present;
    const request = toExportRequest(timeline, m.session, m.path, mode, fit20);
    const visit = this.visit;
    this.busy = true;
    this.handing = mode;
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
      this.exported.set(id, timeline);
      this.log.push({ action: mode, id, input: m.path, visit });
      this.track(id);
    } finally {
      this.handing = null;
      this.busy = false;
    }
  }

  /** Next video: the next newer clip (see `go`). */
  nextVideo(): Promise<void> {
    return this.go('next');
  }

  /** Prev: the next older clip (see `go`). */
  prevVideo(): Promise<void> {
    return this.go('prev');
  }

  /**
   * Moves to the next clip `dir`'s way: through the queue, then the folder past its end. It asks
   * first when the clip has edits no export was made from; a running export carries on by itself.
   */
  private async go(dir: Direction): Promise<void> {
    const from = this.current;
    if (!from || this.busy || !this.canGo(dir)) return;
    this.busy = true;
    try {
      if (this.unsaved && !(await this.ask('move'))) return;
      let to: Current | null;
      try {
        to = await this.step(from, dir);
      } catch (err) {
        this.notice = plain(err, LOOK_FAILED[dir]);
        return;
      }
      // The folder changed since it was checked: there's nothing that way after all.
      if (!to) {
        this.ahead = { ...this.ahead, [dir]: null };
        this.aheadUnknown = { ...this.aheadUnknown, [dir]: false };
        return;
      }
      void this.leave();
      await this.show(to);
    } finally {
      this.busy = false;
    }
  }

  /**
   * Sends the file to the Recycle Bin and moves on as Next would, or as Prev would when there's
   * nothing newer; an edited clip asks first.
   */
  async discard(): Promise<void> {
    const from = this.current;
    if (!from || this.busy || this.locked) return;
    const path = from.path;
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
        await this.open(path);
        this.notice = plain(err, DISCARD_FAILED);
        return;
      }
      if (after.length === 0) this.markGone(path);
      let to: Current | null = null;
      let why: string | null = null;
      for (const dir of ['next', 'prev'] as const) {
        try {
          to = await this.step(from, dir);
        } catch (err) {
          why ??= plain(err, LOOK_FAILED[dir]);
        }
        if (to) break;
      }
      if (to) {
        await this.show(to);
        // Gone older because the newer look failed: say so (the open cleared the status bar).
        if (why) this.notice = why;
      } else {
        this.notice = why;
        // Nothing left to trim: the window stays, with the results list.
        this.current = null;
        this.visit += 1;
        this.ahead = { next: undefined, prev: undefined };
        this.aheadUnknown = { next: false, prev: false };
      }
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
        void this.addFiles(e.paths);
        return;
      case 'ExportStarted':
        this.track(e.id);
        if (x.running.includes(e.id) && !(e.id in x.progress)) x.progress[e.id] = 0;
        return;
      case 'ExportProgress':
        if (x.running.includes(e.id)) x.progress[e.id] = e.fraction;
        return;
      case 'ExportDone': {
        this.finish(e.id);
        x.done.push({ id: e.id, input: e.input, output: e.output, bytes: e.bytes, note: e.note });
        // A Quick / Save raw export has recycled or moved its original by now.
        const entry = this.log.find((l) => 'id' in l && l.id === e.id);
        if (entry && (entry.action === 'quick' || entry.action === 'save_raw')) this.markGone(entry.input);
        this.refreshAhead();
        return;
      }
      case 'ExportFailed':
        this.finish(e.id);
        x.failed.push({ id: e.id, input: e.input, message: e.message, originalIntact: e.original_intact });
        // Its original stayed: Prev / Next may reach it again.
        this.refreshAhead();
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
        else this.markGone(e.path);
        this.refreshAhead();
        return;
      }
    }
  }

  /** Puts `to` on screen as a new visit and opens it. */
  private show(to: Current): Promise<void> {
    if (to.at !== null) this.queue.next = Math.max(this.queue.next, to.at + 1);
    this.current = to;
    this.visit += 1;
    return this.open(to.path);
  }

  /** Opens `path` and loads it; thumbnails and waveforms fill in as they arrive (spec 6). */
  private async open(path: string): Promise<void> {
    const seq = ++this.openSeq;
    this.error = null;
    this.ahead = { next: undefined, prev: undefined };
    this.aheadUnknown = { next: false, prev: false };
    let media: EditorMedia;
    try {
      media = await api.open(path);
    } catch (err) {
      if (seq !== this.openSeq) return;
      this.error = plain(err, OPEN_FAILED);
      await this.lookAhead(seq);
      return;
    }
    if (seq !== this.openSeq || this.currentPath !== path) {
      void this.closeSession(media.session); // moved on while it opened
      return;
    }
    this.modified.set(media.path, media.modified_ms);
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
    await this.lookAhead(seq);
  }

  /**
   * Looks ahead for Prev's and Next's disabled states: only needed for a way the queue has no
   * step of its own (the queue's steps are known without asking).
   */
  private async lookAhead(seq: number): Promise<void> {
    const from = this.current;
    if (!from) return;
    const mine = ++this.lookSeq;
    const latest = () => seq === this.openSeq && mine === this.lookSeq;
    const look = async (dir: Direction) => {
      if (this.queueStep(from, dir)) return;
      try {
        const to = await this.folderStep(from, dir);
        if (!latest()) return;
        this.ahead = { ...this.ahead, [dir]: to };
        this.aheadUnknown = { ...this.aheadUnknown, [dir]: false };
      } catch (err) {
        console.error('editor_adjacent_file failed', err);
        if (latest()) this.aheadUnknown = { ...this.aheadUnknown, [dir]: true };
      }
    };
    await Promise.all([look('next'), look('prev')]);
  }

  /**
   * Looks ahead again for the open clip: an export or discard that ended can change which clips
   * are on their way out (one whose export failed is back, say). A clip still opening runs its own.
   */
  private refreshAhead(): void {
    if (this.current && (this.media || this.error)) void this.lookAhead(this.openSeq);
  }

  private canGo(dir: Direction): boolean {
    const from = this.current;
    if (!from) return false;
    return this.queueStep(from, dir) !== null || !!this.ahead[dir] || this.aheadUnknown[dir];
  }

  /** For the tooltip and disabled state only: `go` always works the step out afresh. */
  private target(dir: Direction): Current | null | undefined {
    const from = this.current;
    if (!from) return null;
    return this.queueStep(from, dir) ?? this.ahead[dir];
  }

  /** Where Prev / Next go from `from`: a queued file, else the folder's neighbour that way. */
  private async step(from: Current, dir: Direction): Promise<Current | null> {
    return this.queueStep(from, dir) ?? (await this.folderStep(from, dir));
  }

  /**
   * The queued file Prev / Next go to from `from`, passing over files on their way out or gone;
   * null when that way leads into the folder. From a queued file the step is to its neighbour.
   * From a folder clip past the newer end, Next goes to the first queued file not opened yet
   * (more arrived meanwhile); every other folder step is the folder's.
   */
  private queueStep(from: Current, dir: Direction): Current | null {
    const q = this.queue;
    let at: number;
    if (from.at !== null) at = from.at;
    else if (dir === 'next' && from.side === 'newer') at = q.next - 1;
    else return null;
    const skip = this.unavailable();
    const by = dir === 'next' ? 1 : -1;
    for (let i = at + by; i >= 0 && i < q.files.length; i += by) {
      const p = q.files[i];
      if (p !== from.path && !skip.has(p)) return { path: p, at: i };
    }
    return null;
  }

  /**
   * The folder's next newer (Next) or older (Prev) video from `from`, passing over clips on their
   * way out (a Quick / Save raw export or a discard still to finish). One that's queued takes its
   * place in the queue, so walking back from the folder carries on through the queue. Null when
   * there's nothing that way, or nothing known about `from` (it never opened).
   */
  private async folderStep(from: Current, dir: Direction): Promise<Current | null> {
    const newer = dir === 'next';
    const leaving = this.leaving();
    let path = from.path;
    for (let i = 0; i < MAX_PASSED; i++) {
      const ms = this.modified.get(path);
      if (ms === undefined) return null;
      const found = await api.adjacentFile(path, ms, newer);
      if (found === null) return null;
      if (!leaving.has(found)) {
        const files = this.queue.files;
        const at = newer ? files.indexOf(found) : files.lastIndexOf(found);
        if (at >= 0) return { path: found, at };
        // Past the queue's newer or older end; a folder clip stays on the side it was on.
        return { path: found, at: null, side: from.at === null ? from.side : newer ? 'newer' : 'older' };
      }
      path = found;
    }
    return null;
  }

  /** Files Prev / Next pass over in the queue: on their way out, or gone. */
  private unavailable(): Set<string> {
    return new Set([...this.leaving(), ...this.gone]);
  }

  private markGone(path: string): void {
    if (!this.gone.has(path)) this.gone = new Set([...this.gone, path]);
  }

  /** Files waiting to be recycled or moved: as good as gone, so they're never opened again. */
  private leaving(): Set<string> {
    const running = new Set(this.exports.running);
    return new Set(
      this.log.flatMap((l) =>
        (l.action === 'discard' && l.state === 'waiting') ||
        ((l.action === 'quick' || l.action === 'save_raw') && running.has(l.id))
          ? [l.input]
          : [],
      ),
    );
  }

  /** The open clip's latest export of the kind `pick` accepts, in this visit. */
  private latest(pick: (action: ExportMode) => boolean): Result | null {
    for (let i = this.log.length - 1; i >= 0; i--) {
      const l = this.log[i];
      if ('id' in l && l.visit === this.visit && pick(l.action)) return this.resultOf(l);
    }
    return null;
  }

  /** An export being handed over reads as queued. */
  private handover(action: ExportMode): Result {
    return { input: this.currentPath ?? '', action, state: 'queued', fraction: 0 };
  }

  private inFlight(r: Result | null): boolean {
    return r !== null && (r.state === 'queued' || r.state === 'running');
  }

  private failedJob(id: number): boolean {
    return this.exports.failed.some((f) => f.id === id);
  }

  private resultOf(entry: LogEntry): Result {
    const e = this.exports;
    if (entry.action === 'discard') {
      const state = entry.state === 'done' ? 'none' : entry.state === 'kept' ? 'failed' : 'running';
      return { input: entry.input, action: 'discard', state, fraction: 0, message: entry.message };
    }
    const { id, input, action } = entry;
    const done = e.done.find((d) => d.id === id);
    if (done) return { input, action, state: 'done', fraction: 1, output: done.output, bytes: done.bytes, note: done.note };
    const failed = e.failed.find((f) => f.id === id);
    if (failed) {
      const { message, originalIntact } = failed;
      return { input, action, state: 'failed', fraction: 0, message, originalIntact };
    }
    if (id in e.progress) return { input, action, state: 'running', fraction: e.progress[id] };
    return { input, action, state: 'queued', fraction: 0 };
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
