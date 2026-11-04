// Dev-only: feeds scripted job events to the progress window when the Tauri API is absent.
// Usage: `npm run dev`, then http://localhost:1420/progress.html?mock=running (also: confirm,
// failed, unchanged, done, noted, batch, convert). Imported dynamically from main.ts behind
// `import.meta.env.DEV`.

import type { JobEvent } from '../lib/jobs';

const file = 'C:\\Videos\\[CLIPPED] Replay 2026-10-09 23-40-17.mp4';
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
  running: [{ type: 'Queued', snapshot: { current_index: 1, total: 1 } }, started(plan), progress(0.384)],
  batch: [{ type: 'Queued', snapshot: { current_index: 3, total: 12 } }, started(plan), progress(0.384)],
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
    { type: 'Failed', id: 1, message: "Couldn't process this video.", original_intact: true },
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
    {
      type: 'Done',
      id: 1,
      output: 'C:\\Videos\\[20MB] Replay 2026-10-09 23-40-17.mp4',
      bytes: 19_400_000,
    },
    { type: 'Drained' },
  ],
  // An editor export saved after its window closed, whose 20 MB copy wasn't made.
  noted: [
    started({ kind: 'export', input: 'C:\\clips\\Replay 2026-10-09 23-40-17.mp4', input_bytes: 0 }),
    {
      type: 'Done',
      id: 1,
      output: 'C:\\clips\\[CLIPPED] Replay 2026-10-09 23-40-17.mp4',
      bytes: 190_000_000,
      note: 'Saved "[CLIPPED] Replay 2026-10-09 23-40-17.mp4", but couldn\'t make the 20 MB copy: Couldn\'t find your Videos folder.',
    },
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
