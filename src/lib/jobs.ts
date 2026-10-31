// TypeScript mirror of the Rust job-queue contract (src-tauri/src/jobs/queue.rs).
// `JobEvent` is serde `#[serde(tag = "type")]`, so it is a union discriminated on `type`.
// Field names stay snake_case, exactly as the Rust structs serialise them.
import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

export const JOB_EVENT = 'hasami://job';

/** "File {current_index} of {total}". */
export interface QueueSnapshot {
  current_index: number;
  total: number;
}

export type JobEvent =
  | { type: 'Queued'; snapshot: QueueSnapshot }
  | {
      type: 'Started';
      id: number;
      input: string;
      input_bytes: number;
      // Optional display hints. Not part of the Plan 1 Rust contract; the window
      // falls back to its `?kind=` query when they are absent (see state.ts).
      kind?: 'compress' | 'convert';
      ext?: string;
      width?: number;
      height?: number;
      fps?: number;
    }
  | { type: 'Progress'; id: number; pass: number; passes: number; fraction: number; eta_s: number | null }
  | { type: 'NeedsConfirm'; id: number; width: number; height: number; fps: number }
  | { type: 'Done'; id: number; output: string; bytes: number }
  | { type: 'Unchanged'; id: number; reason: string }
  | { type: 'Failed'; id: number; message: string }
  | { type: 'Drained' };

export function onJobEvent(cb: (event: JobEvent) => void): Promise<UnlistenFn> {
  return listen<JobEvent>(JOB_EVENT, (e) => cb(e.payload));
}

export function cancelJob(id: number): Promise<void> {
  return invoke('cancel_job', { id });
}

export function confirmLowRes(id: number, proceed: boolean): Promise<void> {
  return invoke('confirm_low_res', { id, proceed });
}
