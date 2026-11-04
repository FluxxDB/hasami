// What the editor has handed to the Rust job queue, and how each piece of it stands (spec 5.8):
// the exports still running, the ones done or failed, and the log of everything done to each file
// (exports and discards, in order) that the results list and the top bar read.
//
// The store (store.svelte.ts) records into it as it hands work over and applies the Rust side's
// events to it; everything here is bookkeeping, with no calls of its own.

import type { EditorEvent } from './api';
import type { ExportRequest, Timeline } from './timeline';

export type ExportMode = ExportRequest['mode'];

export type LogEntry =
  /** `visit` is the stay on the clip it was started from (see `EditorStore.visit`). */
  | { action: ExportMode; id: number; input: string; visit: number }
  /**
   * A discard of a clip with exports still to finish (an Export audio still queued reads the
   * file) is carried out by the Rust side once they're done; `Discard*` events report it.
   */
  | { action: 'discard'; input: string; state: 'waiting' | 'done' | 'kept'; message?: string };

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

export interface Exports {
  /** Job ids handed to the queue and not yet finished. */
  running: number[];
  done: { id: number; input: string; output: string; bytes: number; note?: string }[];
  failed: { id: number; input: string; message: string; originalIntact: boolean }[];
  /** Latest progress (0..1) of each export the queue has started (absent: still queued). */
  progress: Record<number, number>;
}

/** Still to finish: queued or running. */
export function inFlight(r: Result | null): boolean {
  return r !== null && (r.state === 'queued' || r.state === 'running');
}

const movesOriginal = (action: LogEntry['action']): boolean => action === 'quick' || action === 'save_raw';

export class ExportLog {
  exports = $state<Exports>({ running: [], done: [], failed: [], progress: {} });
  /** What was done to each file, in order, for the results list. */
  log = $state<LogEntry[]>([]);

  /** Exports already finished, in case their last event beats `editor_export`'s answer. */
  private finished = new Set<number>();
  /** The timeline each export was made from, so moving on and closing know which edits went out. */
  private madeFrom = new Map<number, Timeline>();

  /** The results list's lines, in the order things were done. */
  get results(): Result[] {
    return this.log.map((entry) => this.resultOf(entry));
  }

  /** Records export `id` of `input`, made from `timeline` during `visit`, and follows it. */
  addExport(action: ExportMode, id: number, input: string, visit: number, timeline: Timeline): void {
    this.madeFrom.set(id, timeline);
    this.log.push({ action, id, input, visit });
    this.track(id);
  }

  /**
   * Records a discard of `input`: `waiting` when the Rust side carries it out after the clip's
   * exports. Logged before it's asked for, as the outcome may arrive first; the returned function
   * takes it back if the ask fails.
   */
  addDiscard(input: string, waiting: boolean): () => void {
    const at = this.log.length;
    this.log.push({ action: 'discard', input, state: waiting ? 'waiting' : 'done' });
    return () => void this.log.splice(at, 1);
  }

  /**
   * Applies an export or discard event. Returns the file it says is gone (recycled, or moved by a
   * Quick / Save raw export), if any, and whether anything ended (so Prev / Next look again).
   */
  apply(e: EditorEvent): { gone: string | null; ended: boolean } {
    const x = this.exports;
    switch (e.type) {
      case 'ExportStarted':
        this.track(e.id);
        if (x.running.includes(e.id) && !(e.id in x.progress)) x.progress[e.id] = 0;
        return { gone: null, ended: false };
      case 'ExportProgress':
        if (x.running.includes(e.id)) x.progress[e.id] = e.fraction;
        return { gone: null, ended: false };
      case 'ExportDone': {
        this.finish(e.id);
        x.done.push({ id: e.id, input: e.input, output: e.output, bytes: e.bytes, note: e.note });
        // A Quick / Save raw export has recycled or moved its original by now.
        const entry = this.log.find((l) => 'id' in l && l.id === e.id);
        return { gone: entry && movesOriginal(entry.action) ? entry.input : null, ended: true };
      }
      case 'ExportFailed':
        this.finish(e.id);
        x.failed.push({ id: e.id, input: e.input, message: e.message, originalIntact: e.original_intact });
        // Its original usually stayed, and Prev / Next may reach it again; but not when it was
        // recycled and the new file then couldn't take its name (spec 8.3).
        return { gone: e.original_intact ? null : e.input, ended: true };
      case 'DiscardDone':
      case 'DiscardKept': {
        const entry = this.log.find((l) => l.action === 'discard' && l.input === e.path && l.state === 'waiting');
        if (entry?.action !== 'discard') return { gone: null, ended: false };
        entry.state = e.type === 'DiscardDone' ? 'done' : 'kept';
        if (e.type === 'DiscardKept') entry.message = e.message;
        return { gone: e.type === 'DiscardDone' ? e.path : null, ended: true };
      }
      default:
        return { gone: null, ended: false };
    }
  }

  /** The latest export of the kind `pick` accepts, started during `visit`; null when there's none. */
  latest(visit: number, pick: (action: ExportMode) => boolean): Result | null {
    for (let i = this.log.length - 1; i >= 0; i--) {
      const l = this.log[i];
      if ('id' in l && l.visit === visit && pick(l.action)) return this.resultOf(l);
    }
    return null;
  }

  /** Some export started during `visit` (and not failed) was made from exactly `timeline`. */
  exportedDuring(visit: number, timeline: Timeline): boolean {
    return this.log.some(
      (l) => 'id' in l && l.visit === visit && this.madeFrom.get(l.id) === timeline && !this.failedJob(l.id),
    );
  }

  /** Files waiting to be recycled or moved: as good as gone, so they're never opened again. */
  leaving(): Set<string> {
    const running = new Set(this.exports.running);
    return new Set(
      this.log.flatMap((l) =>
        (l.action === 'discard' && l.state === 'waiting') || ('id' in l && movesOriginal(l.action) && running.has(l.id))
          ? [l.input]
          : [],
      ),
    );
  }

  /** The job ids of exports of `path` still queued or running. */
  runningExportsOf(path: string): number[] {
    const running = new Set(this.exports.running);
    return this.log.flatMap((l) => ('id' in l && l.input === path && running.has(l.id) ? [l.id] : []));
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
}
