// Dev-only: feeds scripted job events to the progress window when the Tauri API is absent.
// Usage: http://localhost:5181/progress.html?mock=running  (also: confirm, failed, unchanged,
// done, batch, convert). Imported dynamically from main.ts behind `import.meta.env.DEV`.
import type { JobEvent } from '../lib/jobs';

const file = 'C:\\Users\\ledan\\Videos\\[CLIPPED] Replay 2026-10-09 23-40-17.mp4';
const started = (extra: Partial<Extract<JobEvent, { type: 'Started' }>> = {}): JobEvent => ({
  type: 'Started',
  id: 1,
  input: file,
  input_bytes: 412_000_000,
  ...extra,
});
const progress = (fraction: number, pass = 1, eta_s: number | null = 24): JobEvent => ({
  type: 'Progress',
  id: 1,
  pass,
  passes: 2,
  fraction,
  eta_s,
});
const plan = { width: 1280, height: 720, fps: 60 };

export const scripts: Record<string, JobEvent[]> = {
  running: [
    { type: 'Queued', snapshot: { current_index: 1, total: 1 } },
    started(plan),
    progress(0.384),
  ],
  batch: [
    { type: 'Queued', snapshot: { current_index: 3, total: 12 } },
    started(plan),
    progress(0.384),
  ],
  convert: [
    { type: 'Queued', snapshot: { current_index: 1, total: 1 } },
    started({ kind: 'convert', ext: 'mov' }),
    { type: 'Progress', id: 1, pass: 1, passes: 1, fraction: 0.62, eta_s: 9 },
  ],
  confirm: [
    { type: 'Queued', snapshot: { current_index: 1, total: 1 } },
    started(),
    { type: 'NeedsConfirm', id: 1, width: 426, height: 240, fps: 30 },
  ],
  failed: [
    { type: 'Queued', snapshot: { current_index: 1, total: 1 } },
    started(plan),
    progress(0.2),
    { type: 'Failed', id: 1, message: 'ffmpeg could not read the audio stream (exit code 1).' },
    { type: 'Drained' },
  ],
  unchanged: [
    { type: 'Queued', snapshot: { current_index: 1, total: 1 } },
    started(plan),
    { type: 'Unchanged', id: 1, reason: 'This file is already under 20 MB.' },
    { type: 'Drained' },
  ],
  done: [
    { type: 'Queued', snapshot: { current_index: 1, total: 1 } },
    started(plan),
    progress(1, 2, 0),
    { type: 'Done', id: 1, output: 'C:\\Users\\ledan\\Videos\\[CLIPPED] Replay [20MB].mp4', bytes: 19_400_000 },
    { type: 'Drained' },
  ],
};

export function mockListen(name: string) {
  const script = scripts[name] ?? scripts.running;
  return async (cb: (e: JobEvent) => void) => {
    const timers = script.map((event, i) => setTimeout(() => cb(event), 60 * (i + 1)));
    return () => timers.forEach(clearTimeout);
  };
}
