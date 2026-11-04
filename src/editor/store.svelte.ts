// The Quick Trim editor's state: the clip being edited (clipEditor.svelte.ts), and the trim queue
// around it (spec 5.7, 5.8). Components render from it.
//
// The queue: which file is open, the exports handed to the Rust job queue and how they're going
// (exportLog.svelte.ts), and the results list. The window never closes by itself: a clip stays
// open while it exports (a Quick / Save raw export locks it), and Prev / Next move on. The queue
// is ordered by date, oldest first, and opens on the oldest: Next goes to the next newer clip and
// Prev to the next older one, through the queued files and then on into the folder past either
// end (navigation.ts). Leaving a clip (Prev, Next, Discard) closes its editor session on the Rust
// side.

import { api, type EditorEvent, type EditorMedia } from './api';
import { ClipEditor } from './clipEditor.svelte';
import { ExportLog, inFlight, type ExportMode, type Exports, type LogEntry, type Result } from './exportLog.svelte';
import { folderStep, queueStep, type Current, type Direction, type Queue } from './navigation';
import { toExportRequest } from './timeline';

/** What the unsaved-edits dialog is asking about: closing, moving on (Prev / Next), or Discard. */
export type ConfirmKind = 'close' | 'move' | 'discard';

// Shown when a command fails without a message of its own.
const START_FAILED = "The editor couldn't start. Close this window and try Quick Trim again.";
const OPEN_FAILED = "Couldn't open this clip.";
const EXPORT_FAILED = "Couldn't start the export.";
const DISCARD_FAILED = "Couldn't send this clip to the Recycle Bin.";
const LOOK_FAILED: Record<Direction, string> = {
  next: "Couldn't look for the next video in this folder.",
  prev: "Couldn't look for the previous video in this folder.",
};

/** The Rust commands answer failures with a plain-language string. */
const plain = (err: unknown, fallback: string): string => (typeof err === 'string' && err ? err : fallback);

export class EditorStore extends ClipEditor {
  /** The 20 MB copy toggle: video exports also save a ≤ 20 MB copy to Videos. */
  fit20 = $state(false);
  /**
   * The files Hasami was asked to trim, oldest first. `next` is the first not opened yet; the
   * files from there on are re-sorted when more arrive.
   */
  queue = $state<Queue>({ files: [], next: 0 });
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
  /** The export being handed to the job queue (until `editor_export` answers); null otherwise. */
  handing = $state<ExportMode | null>(null);

  /**
   * Why the open clip couldn't be discarded: said under the top bar, where it can't be missed,
   * until the next clip. `seq` tells one failure from the next with the same words.
   */
  discardFailure = $state<{ message: string; seq: number } | null>(null);
  private discardFailures = 0;
  /** The unsaved-edits question being asked, if any; `answer` replies. */
  confirm = $state<ConfirmKind | null>(null);
  /** An export hand-over, Next or Discard is under way; the buttons wait for it. */
  busy = $state(false);

  /** The exports handed to the job queue, and everything done to each file (exportLog.svelte.ts). */
  private readonly jobs = new ExportLog();

  /** Called after each clip loads (the browser mock applies its demo edits here). */
  afterLoad: ((store: EditorStore) => void) | null = null;

  private reply: ((ok: boolean) => void) | null = null;
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

  /** The open clip's latest Quick / Save raw export, with how it stands; null when there's none. */
  get videoExport(): Result | null {
    if (this.handing === 'quick' || this.handing === 'save_raw') return this.handover(this.handing);
    return this.jobs.latest(this.visit, (a) => a !== 'audio');
  }

  /** The open clip's latest Export audio, with how it stands; null when there's none. */
  get audioExport(): Result | null {
    if (this.handing === 'audio') return this.handover('audio');
    return this.jobs.latest(this.visit, (a) => a === 'audio');
  }

  /**
   * A Quick / Save raw export of the open clip is on its way or done, so its original is about
   * to move (or has): no more edits. A failed one unlocks the clip to try again.
   */
  override get locked(): boolean {
    const v = this.videoExport;
    return v !== null && v.state !== 'failed';
  }

  /** The open clip has edits that no export (on its way or done) was made from. */
  get unsaved(): boolean {
    return this.edited && !this.jobs.exportedDuring(this.visit, this.history.present);
  }

  /** The exports handed to the job queue: running, done and failed. */
  get exports(): Exports {
    return this.jobs.exports;
  }

  /** What was done to each file, in order. */
  get log(): LogEntry[] {
    return this.jobs.log;
  }

  /** The results list's lines, in the order things were done. */
  get results(): Result[] {
    return this.jobs.results;
  }

  // --- queue -------------------------------------------------------------------------------

  /**
   * Starts the window: listens for the Rust side's events, tells it so (it holds them until then),
   * and opens the files it was asked to trim; later batches arrive as `Files` events.
   */
  async start(): Promise<void> {
    let files: string[];
    try {
      await api.onEvent((e) => this.applyEvent(e));
      files = await api.files();
    } catch (err) {
      console.error('The editor could not start', err);
      this.error = START_FAILED;
      return;
    }
    const opening = this.addFiles(files);
    api.ready().catch((err) => console.error('editor_ready failed', err));
    await opening.catch((err) => console.error('Opening the first clip failed', err));
  }

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
    const pending = new Set([...ahead, ...(this.current ? [this.current.path] : []), ...this.jobs.leaving()]);
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
    if (mode === 'audio' && (!this.canExportAudio || inFlight(this.audioExport))) return;
    this.commitLive();
    const fit20 = mode !== 'audio' && this.fit20;
    const timeline = this.history.present;
    const request = toExportRequest(timeline, m.session, m.path, mode, fit20, this.crop);
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
      this.jobs.addExport(mode, id, m.path, visit, timeline);
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
      const after = this.jobs.runningExportsOf(path);
      // Logged before asking: Rust may report the outcome (Discard*) before its reply arrives.
      const unlog = this.jobs.addDiscard(path, after.length > 0);
      try {
        await api.discard(path, after);
      } catch (err) {
        unlog();
        await this.open(path);
        this.discardFailure = { message: plain(err, DISCARD_FAILED), seq: ++this.discardFailures };
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
    switch (e.type) {
      case 'Files':
        this.addFiles(e.paths).catch((err) => console.error('Adding files failed', err));
        return;
      case 'ProxyProgress':
        if (e.session === this.media?.session && this.proxyFraction !== null) this.proxyFraction = e.fraction;
        return;
      default: {
        const { gone, ended } = this.jobs.apply(e);
        if (gone) this.markGone(gone);
        // An export or discard that ended can change which clips are on their way out.
        if (ended) this.refreshAhead();
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

  private queueStep(from: Current, dir: Direction): Current | null {
    return queueStep(this.queue, from, dir, this.unavailable());
  }

  private folderStep(from: Current, dir: Direction): Promise<Current | null> {
    return folderStep(from, dir, {
      files: this.queue.files,
      modified: this.modified,
      leaving: this.jobs.leaving(),
      adjacentFile: (path, ms, newer) => api.adjacentFile(path, ms, newer),
    });
  }

  /** Files Prev / Next pass over in the queue: on their way out, or gone. */
  private unavailable(): Set<string> {
    return new Set([...this.jobs.leaving(), ...this.gone]);
  }

  private markGone(path: string): void {
    if (!this.gone.has(path)) this.gone = new Set([...this.gone, path]);
  }

  /** An export being handed over reads as queued. */
  private handover(action: ExportMode): Result {
    return { input: this.currentPath ?? '', action, state: 'queued', fraction: 0 };
  }

  /**
   * Lets go of the open clip: nothing of it stays on screen, and its session closes (resolves
   * once the Rust side has stopped its work; only Discard needs to wait for that).
   */
  private leave(): Promise<void> {
    this.openSeq += 1; // an open still in flight is no longer wanted
    const closed = this.media ? this.closeSession(this.media.session) : Promise.resolve();
    this.clear();
    this.discardFailure = null;
    return closed;
  }

  private closeSession(session: number): Promise<void> {
    return api.close(session).catch((err) => console.error('editor_close failed', err));
  }

}
