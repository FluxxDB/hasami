import { describe, expect, it } from 'vitest';
import type { JobEvent } from '../lib/jobs';
import { initialState, reduce, type ViewState } from './state';

const run = (events: JobEvent[], kind: 'compress' | 'convert' = 'compress'): ViewState =>
  events.reduce(reduce, initialState(kind));

const started = (id = 1, input = 'C:\\clips\\Replay 01.mp4'): JobEvent => ({
  type: 'Started',
  id,
  input,
  input_bytes: 412_000_000,
});
const progress = (id = 1, fraction = 0.384): JobEvent => ({
  type: 'Progress',
  id,
  pass: 1,
  passes: 2,
  fraction,
  eta_s: 24,
});

describe('reduce', () => {
  it('starts in a waiting phase with the kind from the window query', () => {
    expect(initialState('convert').kind).toBe('convert');
    expect(initialState('convert').phase).toBe('waiting');
  });

  it('shows a running compress job', () => {
    const s = run([{ type: 'Queued', snapshot: { current_index: 1, total: 1 } }, started(), progress()]);
    expect(s.phase).toBe('running');
    expect(s.kind).toBe('compress');
    expect(s.fileName).toBe('Replay 01.mp4');
    expect(s.inputBytes).toBe(412_000_000);
    expect(s.fraction).toBeCloseTo(0.384);
    expect(s.etaS).toBe(24);
    expect(s.pass).toBe(1);
    expect(s.passes).toBe(2);
    expect(s.fileLabel).toBeNull();
  });

  it('keeps the convert kind and takes the extension from the window query', () => {
    const s = reduce(initialState('convert', 'mov'), started());
    expect(s.kind).toBe('convert');
    expect(s.ext).toBe('mov');
  });

  it('lets Started override kind, extension and target when it carries them', () => {
    const s = run([{ ...(started() as Extract<JobEvent, { type: 'Started' }>), kind: 'convert', ext: 'mp4', width: 1280, height: 720, fps: 60 }]);
    expect(s.kind).toBe('convert');
    expect(s.ext).toBe('mp4');
    expect(s.target).toEqual({ width: 1280, height: 720, fps: 60 });
  });

  it('shows "File 3 of 12" only when total > 1', () => {
    const one = run([{ type: 'Queued', snapshot: { current_index: 1, total: 1 } }, started()]);
    expect(one.fileLabel).toBeNull();
    const many = run([{ type: 'Queued', snapshot: { current_index: 3, total: 12 } }, started()]);
    expect(many.fileLabel).toBe('File 3 of 12');
  });

  it('clamps the file index into 1..total', () => {
    const s = run([{ type: 'Queued', snapshot: { current_index: 0, total: 3 } }, started()]);
    expect(s.fileLabel).toBe('File 1 of 3');
    const t = run([{ type: 'Queued', snapshot: { current_index: 9, total: 3 } }, started()]);
    expect(t.fileLabel).toBe('File 3 of 3');
  });

  it('follows a snapshot that grows while running', () => {
    const s = run([
      { type: 'Queued', snapshot: { current_index: 1, total: 1 } },
      started(),
      { type: 'Queued', snapshot: { current_index: 1, total: 2 } },
    ]);
    expect(s.fileLabel).toBe('File 1 of 2');
  });

  it('ignores progress for a different job id', () => {
    const s = run([started(2), progress(1, 0.9)]);
    expect(s.fraction).toBe(0);
  });

  it('asks for confirmation, then resumes on Progress', () => {
    const asking = run([started(), { type: 'NeedsConfirm', id: 1, width: 426, height: 240, fps: 30 }]);
    expect(asking.phase).toBe('confirm');
    expect(asking.target).toEqual({ width: 426, height: 240, fps: 30 });
    expect(reduce(asking, progress()).phase).toBe('running');
  });

  it('keeps the window open on Failed and shows the message', () => {
    const s = run([started(), { type: 'Failed', id: 1, message: 'ffmpeg exited with code 1' }]);
    expect(s.phase).toBe('failed');
    expect(s.failure).toEqual({ fileName: 'Replay 01.mp4', message: 'ffmpeg exited with code 1' });
    expect(s.shouldClose).toBe(false);
  });

  it('Drained after success asks to close', () => {
    const s = run([started(), progress(), { type: 'Done', id: 1, output: 'C:\\clips\\Replay 01 [20MB].mp4', bytes: 19_400_000 }, { type: 'Drained' }]);
    expect(s.phase).toBe('done');
    expect(s.outputBytes).toBe(19_400_000);
    expect(s.shouldClose).toBe(true);
  });

  it('Drained after a failure does not close and shows the failure', () => {
    const s = run([
      started(1, 'C:\\a.mp4'),
      { type: 'Failed', id: 1, message: 'Disk full' },
      started(2, 'C:\\b.mp4'),
      { type: 'Done', id: 2, output: 'C:\\b2.mp4', bytes: 1 },
      { type: 'Drained' },
    ]);
    expect(s.phase).toBe('failed');
    expect(s.failure).toEqual({ fileName: 'a.mp4', message: 'Disk full' });
    expect(s.failedCount).toBe(1);
    expect(s.shouldClose).toBe(false);
  });

  it('a Cancelled job is not a failure: next job runs, then the window closes', () => {
    const mid = run([started(1), { type: 'Failed', id: 1, message: 'Cancelled' }]);
    expect(mid.phase).toBe('cancelled');
    expect(mid.failedCount).toBe(0);
    const end = [started(2), { type: 'Done', id: 2, output: 'x', bytes: 1 } as JobEvent, { type: 'Drained' } as JobEvent].reduce(reduce, mid);
    expect(end.shouldClose).toBe(true);
  });

  it('shows Unchanged with the reason and stays open when nothing was produced', () => {
    const s = run([started(), { type: 'Unchanged', id: 1, reason: 'Already under 20 MB' }, { type: 'Drained' }]);
    expect(s.phase).toBe('unchanged');
    expect(s.notice).toEqual({ fileName: 'Replay 01.mp4', reason: 'Already under 20 MB' });
    expect(s.shouldClose).toBe(false);
  });

  it('closes after Drained when an Unchanged file sat in a batch that produced output', () => {
    const s = run([
      started(1),
      { type: 'Unchanged', id: 1, reason: 'Already under 20 MB' },
      started(2),
      { type: 'Done', id: 2, output: 'x', bytes: 1 },
      { type: 'Drained' },
    ]);
    expect(s.shouldClose).toBe(true);
  });

  it('a new batch after Drained starts clean', () => {
    const s = run([started(1), { type: 'Failed', id: 1, message: 'boom' }, { type: 'Drained' }, started(2)]);
    expect(s.phase).toBe('running');
    expect(s.failedCount).toBe(0);
    expect(s.failure).toBeNull();
    expect(s.shouldClose).toBe(false);
  });

  it('splits file names on both slash styles', () => {
    expect(run([started(1, '/home/x/a b.mkv')]).fileName).toBe('a b.mkv');
  });
});
