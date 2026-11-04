import { describe, expect, it } from 'vitest';
import { ExportLog, inFlight } from './exportLog.svelte';
import { createTimeline } from './timeline';

const CLIP = 'C:\\Videos\\clip.mp4';
const tl = createTimeline(90, 60, [0]);

describe('ExportLog', () => {
  it('follows an export from queued to running to done, and says a Quick export moved its original', () => {
    const log = new ExportLog();
    log.addExport('quick', 1, CLIP, 1, tl);
    expect(log.results).toMatchObject([{ action: 'quick', state: 'queued' }]);
    expect(log.leaving()).toEqual(new Set([CLIP]));
    log.apply({ type: 'ExportStarted', id: 1, input: CLIP });
    log.apply({ type: 'ExportProgress', id: 1, fraction: 0.5 });
    expect(log.results).toMatchObject([{ state: 'running', fraction: 0.5 }]);
    const ended = log.apply({ type: 'ExportDone', id: 1, input: CLIP, output: 'out.mp4', bytes: 10 });
    expect(ended).toEqual({ gone: CLIP, ended: true });
    expect(log.results).toMatchObject([{ state: 'done', output: 'out.mp4', bytes: 10 }]);
    expect(log.exports.running).toEqual([]);
    expect(log.leaving()).toEqual(new Set());
  });

  it("an Export audio leaves its original where it is; a failure says whether it's still there", () => {
    const log = new ExportLog();
    log.addExport('audio', 1, CLIP, 1, tl);
    expect(log.leaving()).toEqual(new Set());
    expect(log.apply({ type: 'ExportDone', id: 1, input: CLIP, output: 'a.mp3', bytes: 1 }).gone).toBeNull();
    log.addExport('quick', 2, CLIP, 1, tl);
    const failed = { type: 'ExportFailed', id: 2, input: CLIP, message: 'No.', original_intact: false } as const;
    expect(log.apply(failed)).toEqual({ gone: CLIP, ended: true });
  });

  it('an export that finishes before it was recorded is not followed again', () => {
    const log = new ExportLog();
    log.apply({ type: 'ExportDone', id: 1, input: CLIP, output: 'out.mp4', bytes: 10 });
    log.addExport('quick', 1, CLIP, 1, tl);
    expect(log.exports.running).toEqual([]);
    expect(log.results[0].state).toBe('done');
  });

  it('knows which edits went out: an export of exactly this timeline in this visit, not failed', () => {
    const log = new ExportLog();
    log.addExport('audio', 1, CLIP, 3, tl);
    expect(log.exportedDuring(3, tl)).toBe(true);
    expect(log.exportedDuring(4, tl)).toBe(false);
    expect(log.exportedDuring(3, createTimeline(90, 60, [0]))).toBe(false);
    log.apply({ type: 'ExportFailed', id: 1, input: CLIP, message: 'No.', original_intact: true });
    expect(log.exportedDuring(3, tl)).toBe(false);
  });

  it('a waiting discard is on its way out until Rust says how it went; taking it back removes it', () => {
    const log = new ExportLog();
    log.addExport('audio', 1, CLIP, 1, tl);
    expect(log.runningExportsOf(CLIP)).toEqual([1]);
    const unlog = log.addDiscard(CLIP, true);
    expect(log.leaving()).toEqual(new Set([CLIP]));
    expect(log.results[1]).toMatchObject({ action: 'discard', state: 'running' });
    unlog();
    expect(log.results).toHaveLength(1);

    log.addDiscard(CLIP, true);
    const kept = log.apply({ type: 'DiscardKept', path: CLIP, message: 'Kept it.' });
    expect(kept).toEqual({ gone: null, ended: true });
    expect(log.results[1]).toMatchObject({ state: 'failed', message: 'Kept it.' });
    // Nothing waiting any more: a second outcome for the same file changes nothing.
    expect(log.apply({ type: 'DiscardDone', path: CLIP })).toEqual({ gone: null, ended: false });
  });

  it('latest picks the newest export of a kind in a visit', () => {
    const log = new ExportLog();
    log.addExport('audio', 1, CLIP, 1, tl);
    log.addExport('quick', 2, CLIP, 1, tl);
    log.addExport('audio', 3, CLIP, 2, tl);
    expect(log.latest(1, (a) => a === 'audio')).toMatchObject({ action: 'audio' });
    expect(log.latest(1, (a) => a !== 'audio')).toMatchObject({ action: 'quick' });
    expect(log.latest(3, () => true)).toBeNull();
    expect(inFlight(log.latest(2, () => true))).toBe(true);
    expect(inFlight(null)).toBe(false);
  });
});
