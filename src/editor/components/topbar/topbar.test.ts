import { describe, expect, it } from 'vitest';
import type { Result } from '../../exportLog.svelte';
import { announcement, borderTitle, outcomeOf, percentText, stepTitle, stripText } from './topbar';

const result = (over: Partial<Result>): Result => ({
  input: 'C:\\Videos\\clip.mp4',
  action: 'quick',
  state: 'queued',
  fraction: 0,
  ...over,
});

describe('outcomeOf', () => {
  it('says why a discard failed before anything else', () => {
    const video = result({ state: 'failed', message: 'Out of disk space.' });
    const r = outcomeOf('C:\\Videos\\clip.mp4', { message: 'In use.' }, video, null);
    expect(r).toMatchObject({ action: 'discard', state: 'failed', message: 'In use.', originalIntact: true });
  });

  it('shows a failed or noted video export, but not a plain saved one (the bar says that)', () => {
    const failed = result({ state: 'failed', message: 'Out of disk space.' });
    expect(outcomeOf('x', null, failed, null)).toBe(failed);
    const noted = result({ state: 'done', note: "Saved, but couldn't make the 20 MB copy." });
    expect(outcomeOf('x', null, noted, null)).toBe(noted);
    expect(outcomeOf('x', null, result({ state: 'done', output: 'y.mp4' }), null)).toBeNull();
  });

  it("shows Export audio's saved file or failure, not its progress", () => {
    const saved = result({ action: 'audio', state: 'done', output: 'clip.mp3' });
    expect(outcomeOf('x', null, null, saved)).toBe(saved);
    expect(outcomeOf('x', null, null, result({ action: 'audio', state: 'running' }))).toBeNull();
  });
});

describe('announcement', () => {
  it('reads out a queued, running, saved or failed export', () => {
    expect(announcement(null)).toBe('');
    expect(announcement(result({}))).toBe('Quick export queued');
    expect(announcement(result({ action: 'audio', state: 'running', fraction: 0.4 }))).toBe('Export audio running');
    expect(announcement(result({ state: 'done', output: 'C:\\Videos\\[CLIPPED] clip.mp4' }))).toBe(
      'Saved [CLIPPED] clip.mp4',
    );
    expect(announcement(result({ state: 'failed', message: 'Out of disk space.', originalIntact: true }))).toBe(
      'Out of disk space. The original was not changed.',
    );
  });
});

describe('labels and tooltips', () => {
  it('a meter reads Queued until it starts, then its percentage', () => {
    expect(percentText(result({}))).toBe('Queued');
    expect(percentText(result({ state: 'running', fraction: 0.384 }))).toBe('38%');
  });

  it('the border tooltip says what video exports do with it', () => {
    const border = { x: 240, y: 135, w: 1440, h: 810 };
    const frame = { width: 1920, height: 1080 };
    expect(borderTitle(border, frame, false)).toContain('crop it off and scale the picture back up to 1920×1080');
    expect(borderTitle(border, frame, true)).toContain('the game fills 1440×810 of the 1920×1080 frame. Video exports keep it.');
  });

  it("Prev / Next name where they go, say when there's nothing that way, and say nothing while checking", () => {
    expect(stepTitle('next', { path: 'C:\\Videos\\b.mp4', at: 1 }, true)).toBe('Next: b.mp4');
    expect(stepTitle('prev', null, true)).toBe('No older video in this folder');
    expect(stepTitle('next', undefined, true)).toBeUndefined();
    expect(stepTitle('next', undefined, false)).toBe('No newer video in this folder');
  });

  it('the export strip counts running exports, then failures', () => {
    expect(stripText(2, 1)).toBe('Exporting 2');
    expect(stripText(0, 1)).toBe('Results · 1 failed');
    expect(stripText(0, 0)).toBe('Results');
  });
});
