// The progress window's state: `reduce` folds each job event (lib/jobs.ts) into what the window
// shows. Pure, so it's unit-tested.

import type { JobEvent } from '../lib/jobs';

/** Shown in the title bar as "HASAMI <kind>". */
export type Kind = 'compress' | 'convert' | 'export';

export type Phase =
  | 'waiting' // window open, no job started yet
  | 'running'
  | 'confirm' // NeedsConfirm: waiting for Continue / Cancel
  | 'done'
  | 'cancelled'
  | 'unchanged'
  | 'noted' // a job was saved, but its Done carries a note (e.g. no 20 MB copy)
  | 'failed';

export interface Target {
  width: number;
  height: number;
  fps: number;
}

export interface ViewState {
  phase: Phase;
  kind: Kind;
  /** Convert target extension without the dot ("mov"). Null when unknown. */
  ext: string | null;
  id: number | null;
  fileName: string;
  inputBytes: number;
  pass: number;
  passes: number;
  fraction: number;
  etaS: number | null;
  /** Output resolution / fps, when known (compress). */
  target: Target | null;
  outputBytes: number | null;
  queue: { index: number; total: number };
  /** "File 3 of 12"; null when there is only one file. */
  fileLabel: string | null;
  /** The failure on screen. One whose original is no longer in place stays over later ones. */
  failure: { fileName: string; message: string; originalIntact: boolean } | null;
  failedCount: number;
  notice: { fileName: string; reason: string } | null;
  noticeCount: number;
  /** A saved job's note. Not a failure: its file was saved, but something else wasn't. */
  note: { fileName: string; message: string } | null;
  doneCount: number;
  drained: boolean;
  /** True once the queue drained with nothing needing attention: close after 1.5 s. */
  shouldClose: boolean;
}

/** `Failed.message` the queue uses for a user cancel; not treated as an error. */
export const CANCELLED = 'Cancelled';

export function initialState(kind: Kind, ext: string | null = null): ViewState {
  return {
    phase: 'waiting',
    kind,
    ext,
    id: null,
    fileName: '',
    inputBytes: 0,
    pass: 1,
    passes: 1,
    fraction: 0,
    etaS: null,
    target: null,
    outputBytes: null,
    queue: { index: 1, total: 1 },
    fileLabel: null,
    failure: null,
    failedCount: 0,
    notice: null,
    noticeCount: 0,
    note: null,
    doneCount: 0,
    drained: false,
    shouldClose: false,
  };
}

export function baseName(path: string): string {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

function fileLabel(index: number, total: number): string | null {
  if (total <= 1) return null;
  // `current_index` is 1-based ("File 3 of 12"); clamp so a stray 0 or overshoot still reads sensibly.
  return `File ${Math.min(total, Math.max(1, index))} of ${total}`;
}

function withQueue(s: ViewState, index: number, total: number): ViewState {
  return { ...s, queue: { index, total }, fileLabel: fileLabel(index, total) };
}

export function reduce(s: ViewState, e: JobEvent): ViewState {
  switch (e.type) {
    case 'Queued':
      return withQueue(s, e.snapshot.current_index, e.snapshot.total);

    case 'Started': {
      // A Started after Drained begins a new batch in a reused window.
      const base = s.drained ? { ...initialState(s.kind, s.ext), queue: s.queue, fileLabel: s.fileLabel } : s;
      return {
        ...base,
        phase: 'running',
        kind: e.kind ?? base.kind,
        ext: e.ext ?? base.ext,
        id: e.id,
        fileName: baseName(e.input),
        inputBytes: e.input_bytes,
        pass: 1,
        passes: 1,
        fraction: 0,
        etaS: null,
        target:
          e.width !== undefined && e.height !== undefined && e.fps !== undefined
            ? { width: e.width, height: e.height, fps: e.fps }
            : null,
        outputBytes: null,
        drained: false,
        shouldClose: false,
      };
    }

    case 'Progress':
      if (e.id !== s.id) return s;
      return {
        ...s,
        phase: 'running',
        pass: e.pass,
        passes: e.passes,
        fraction: e.fraction,
        etaS: e.eta_s,
      };

    case 'NeedsConfirm':
      if (e.id !== s.id) return s;
      return { ...s, phase: 'confirm', target: { width: e.width, height: e.height, fps: e.fps } };

    case 'Done':
      if (e.id !== s.id) return s;
      return {
        ...s,
        phase: e.note ? 'noted' : 'done',
        fraction: 1,
        etaS: null,
        outputBytes: e.bytes,
        doneCount: s.doneCount + 1,
        note: e.note ? { fileName: s.fileName, message: e.note } : s.note,
      };

    case 'Unchanged':
      if (e.id !== s.id) return s;
      return {
        ...s,
        phase: 'unchanged',
        notice: { fileName: s.fileName, reason: e.reason },
        noticeCount: s.noticeCount + 1,
      };

    case 'Failed':
      if (e.id !== s.id) return s;
      if (e.message === CANCELLED) return { ...s, phase: 'cancelled' };
      {
        const failure = { fileName: s.fileName, message: e.message, originalIntact: e.original_intact !== false };
        // Never replace the one message that says where a user's new file ended up.
        const keepEarlier = s.failure !== null && !s.failure.originalIntact;
        return {
          ...s,
          phase: 'failed',
          failure: keepEarlier ? s.failure : failure,
          failedCount: s.failedCount + 1,
        };
      }

    case 'Drained': {
      const drained = { ...s, drained: true };
      // Anything that went wrong stays on screen until the user closes it.
      if (s.failedCount > 0) return { ...drained, phase: 'failed', shouldClose: false };
      if (s.note !== null) return { ...drained, phase: 'noted', shouldClose: false };
      // "Unchanged" with nothing produced would otherwise flash and vanish unexplained.
      if (s.noticeCount > 0 && s.doneCount === 0) {
        return { ...drained, phase: 'unchanged', shouldClose: false };
      }
      return { ...drained, shouldClose: true };
    }
  }
}
