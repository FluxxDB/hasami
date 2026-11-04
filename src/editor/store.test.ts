import { beforeEach, describe, expect, it, vi } from 'vitest';
import { api } from './api';
import { media } from './fixtures';
import { EditorStore } from './store.svelte';
import { setMuted } from './timeline';

// The store talks to the Rust side through `api`; these tests answer for it.
vi.mock('./api', () => ({
  isMock: false,
  api: {
    files: vi.fn(),
    open: vi.fn(),
    peaks: vi.fn(),
    thumbs: vi.fn(),
    proxy: vi.fn(),
    exportClip: vi.fn(),
    discard: vi.fn(),
    adjacentFile: vi.fn(),
    orderFiles: vi.fn(),
    close: vi.fn(),
    ready: vi.fn(),
    onEvent: vi.fn(),
    src: (path: string) => path,
  },
}));

describe('EditorStore queue and exports', () => {
  const FILES = ['C:\\clips\\a.mp4', 'C:\\clips\\b.mp4', 'C:\\clips\\c.mp4'];
  /** The folder's newer videos, after the queued ones. */
  const D = 'C:\\clips\\d.mp4';
  const E = 'C:\\clips\\e.mp4';
  const ALL = [...FILES, D, E];
  const sessionOf = (path: string) => ALL.indexOf(path) + 1;
  // Oldest first: a is the oldest, e the newest.
  const modifiedOf = (path: string) => 10_000 + ALL.indexOf(path);
  let nextJob = 100;

  beforeEach(() => {
    vi.clearAllMocks();
    nextJob = 100;
    vi.mocked(api.open).mockImplementation(async (path) =>
      media({ session: sessionOf(path), path, file_name: path.split('\\').pop()!, modified_ms: modifiedOf(path) }),
    );
    vi.mocked(api.exportClip).mockImplementation(async () => nextJob++);
    vi.mocked(api.discard).mockResolvedValue(undefined);
    vi.mocked(api.adjacentFile).mockResolvedValue(null);
    // Like the Rust side: oldest first.
    vi.mocked(api.orderFiles).mockImplementation(async (paths) =>
      [...paths].sort((x, y) => modifiedOf(x) - modifiedOf(y)),
    );
    vi.mocked(api.close).mockResolvedValue(undefined);
    vi.mocked(api.peaks).mockResolvedValue({ rate: 10, values: [] });
    vi.mocked(api.thumbs).mockResolvedValue([]);
  });

  /** A store with `files` queued and the first one open. */
  const queued = async (files = FILES) => {
    const s = new EditorStore();
    await s.addFiles(files);
    return s;
  };

  /** A folder of a…e in that order (oldest first): `adjacentFile` answers like the Rust side. */
  const folder = (...gone: string[]) =>
    vi.mocked(api.adjacentFile).mockImplementation(async (path, ms, newer) => {
      expect(ms).toBe(modifiedOf(path));
      const at = ALL.indexOf(path);
      const side = newer ? ALL.slice(at + 1) : ALL.slice(0, at).reverse();
      return side.find((p) => !gone.includes(p)) ?? null;
    });
  /** Lets an event's async work (a `Files` merge) run. */
  const settle = () => new Promise((r) => setTimeout(r, 0));

  const cut = (s: EditorStore, t = 12) => {
    s.seek(t);
    s.command('cut');
  };

  // --- exports ---------------------------------------------------------------------------

  it('quick export sends the edit with the 20 MB copy toggle, and the clip stays open and locked', async () => {
    const s = await queued();
    s.fit20 = true;
    cut(s);
    await s.exportClip('quick');
    expect(api.exportClip).toHaveBeenCalledTimes(1);
    const request = vi.mocked(api.exportClip).mock.calls[0][0];
    expect(request).toMatchObject({ session: 1, input: FILES[0], mode: 'quick', fit_20mb: true });
    expect(request.video).toHaveLength(2);
    expect(s.media?.path).toBe(FILES[0]);
    expect(s.current).toEqual({ path: FILES[0], at: 0 });
    expect(api.close).not.toHaveBeenCalled();
    expect(s.exports.running).toEqual([100]);
    expect(s.locked).toBe(true);
    expect(s.edited).toBe(true);
  });

  it('export + save raw sends fit_20mb as the toggle is, and locks too', async () => {
    const s = await queued();
    await s.exportClip('save_raw');
    expect(vi.mocked(api.exportClip).mock.calls[0][0]).toMatchObject({ mode: 'save_raw', fit_20mb: false });
    expect(s.locked).toBe(true);
  });

  it('a locked clip refuses edits, Delete, Ctrl+B, undo and edge drags, and still plays', async () => {
    const s = await queued();
    cut(s, 12);
    s.selectAt(s.timeline.video[1].id);
    await s.exportClip('quick');
    const before = s.history;
    s.seek(30);
    s.command('cut');
    s.command('delete');
    s.command('undo');
    s.toggleMute(0);
    s.setGain(0, -6, false);
    s.setGain(0, -6, true);
    s.beginEdgeDrag(s.timeline.video[0].id, 'end');
    s.dragEdgeTo(5);
    s.endEdgeDrag();
    expect(s.history).toBe(before);
    expect(s.preview).toBeNull();
    expect(s.timeline.video).toHaveLength(2);
    // The transport still works.
    s.command('playPause');
    expect(s.shuttle.dir).toBe(1);
    s.command('home');
    expect(s.playhead).toBe(0);
    // And no second export of a clip that's on its way out.
    await s.exportClip('quick');
    await s.exportClip('audio');
    expect(api.exportClip).toHaveBeenCalledTimes(1);
  });

  it('the clip is locked while the export is being handed over', async () => {
    const s = await queued();
    let reply!: (id: number) => void;
    vi.mocked(api.exportClip).mockImplementationOnce(() => new Promise((r) => (reply = r)));
    cut(s);
    const exporting = s.exportClip('quick');
    expect(s.locked).toBe(true);
    expect(s.videoExport).toMatchObject({ action: 'quick', state: 'queued' });
    s.command('undo');
    expect(s.timeline.video).toHaveLength(2);
    reply(100);
    await exporting;
    expect(s.locked).toBe(true);
  });

  it('a video export reads queued until it starts, then follows its progress, and done shows what was saved', async () => {
    const s = await queued();
    await s.exportClip('quick');
    expect(s.videoExport).toMatchObject({ action: 'quick', state: 'queued', fraction: 0 });
    s.applyEvent({ type: 'ExportStarted', id: 100, input: FILES[0] });
    expect(s.videoExport).toMatchObject({ state: 'running', fraction: 0 });
    s.applyEvent({ type: 'ExportProgress', id: 100, fraction: 0.4 });
    expect(s.videoExport).toMatchObject({ state: 'running', fraction: 0.4 });
    const output = 'C:\\clips\\[CLIPPED] a.mp4';
    s.applyEvent({ type: 'ExportDone', id: 100, input: FILES[0], output, bytes: 5e6 });
    expect(s.videoExport).toMatchObject({ state: 'done', output, bytes: 5e6 });
    expect(s.exports.running).toEqual([]);
    // The original has gone to the Recycle Bin: the clip stays locked, and stays open.
    expect(s.locked).toBe(true);
    expect(s.media?.path).toBe(FILES[0]);
  });

  it('a failed video export says why, and unlocks the clip to try again', async () => {
    const s = await queued();
    cut(s);
    await s.exportClip('quick');
    s.applyEvent({ type: 'ExportStarted', id: 100, input: FILES[0] });
    s.applyEvent({
      type: 'ExportFailed',
      id: 100,
      input: FILES[0],
      message: 'Out of disk space.',
      original_intact: true,
    });
    expect(s.videoExport).toMatchObject({ state: 'failed', message: 'Out of disk space.', originalIntact: true });
    expect(s.exports.failed).toEqual([
      { id: 100, input: FILES[0], message: 'Out of disk space.', originalIntact: true },
    ]);
    expect(s.locked).toBe(false);
    cut(s, 30);
    expect(s.timeline.video).toHaveLength(3);
    await s.exportClip('quick');
    expect(api.exportClip).toHaveBeenCalledTimes(2);
    expect(s.locked).toBe(true);
    expect(s.videoExport).toMatchObject({ state: 'queued' });
  });

  it('export audio shows its progress and leaves the clip editable', async () => {
    const s = await queued();
    s.fit20 = true;
    cut(s);
    await s.exportClip('audio');
    expect(vi.mocked(api.exportClip).mock.calls[0][0]).toMatchObject({ mode: 'audio', video: [], fit_20mb: false });
    expect(s.locked).toBe(false);
    expect(s.videoExport).toBeNull();
    expect(s.audioExport).toMatchObject({ action: 'audio', state: 'queued' });
    s.applyEvent({ type: 'ExportStarted', id: 100, input: FILES[0] });
    s.applyEvent({ type: 'ExportProgress', id: 100, fraction: 0.5 });
    expect(s.audioExport).toMatchObject({ state: 'running', fraction: 0.5 });
    cut(s, 30);
    expect(s.timeline.video).toHaveLength(3);
    // A second Export audio waits for the first; a video export doesn't.
    await s.exportClip('audio');
    expect(api.exportClip).toHaveBeenCalledTimes(1);
    s.applyEvent({ type: 'ExportDone', id: 100, input: FILES[0], output: 'C:\\clips\\[CLIPPED] a.mp3', bytes: 1 });
    expect(s.audioExport).toMatchObject({ state: 'done', output: 'C:\\clips\\[CLIPPED] a.mp3' });
    await s.exportClip('quick');
    expect(api.exportClip).toHaveBeenCalledTimes(2);
  });

  it('an export that finishes before its id comes back is not left running', async () => {
    const s = await queued();
    vi.mocked(api.exportClip).mockImplementationOnce(async () => {
      s.applyEvent({ type: 'ExportStarted', id: 100, input: FILES[0] });
      s.applyEvent({ type: 'ExportDone', id: 100, input: FILES[0], output: 'out.mp3', bytes: 1 });
      return 100;
    });
    await s.exportClip('audio');
    expect(s.exports.running).toEqual([]);
    expect(s.exports.done.map((d) => d.id)).toEqual([100]);
    expect(s.audioExport).toMatchObject({ state: 'done' });
  });

  it("an export that can't be queued says so and leaves the clip as it was", async () => {
    const s = await queued();
    vi.mocked(api.exportClip).mockRejectedValueOnce("Couldn't start the export.");
    cut(s);
    await s.exportClip('quick');
    expect(s.notice).toBe("Couldn't start the export.");
    expect(s.media?.path).toBe(FILES[0]);
    expect(s.edited).toBe(true);
    expect(s.locked).toBe(false);
    expect(s.videoExport).toBeNull();
    expect(s.exports.running).toEqual([]);
  });

  it('canExportAudio is false when every track is muted', async () => {
    const s = await queued();
    expect(s.canExportAudio).toBe(true);
    s.toggleMute(0);
    expect(s.canExportAudio).toBe(true);
    s.edit(setMuted(s.timeline, 2, true));
    expect(s.canExportAudio).toBe(false);
    await s.exportClip('audio');
    expect(api.exportClip).not.toHaveBeenCalled();
  });

  it('canExportAudio is false for a clip with no audio tracks', async () => {
    vi.mocked(api.open).mockResolvedValueOnce(media({ tracks: [{ index: 0, empty: true, file: null }] }));
    const s = await queued([FILES[0]]);
    expect(s.canExportAudio).toBe(false);
  });

  // --- the window stays open ---------------------------------------------------------------

  it('never closes by itself: a single clip stays open after its export is done', async () => {
    const s = await queued([FILES[0]]);
    await s.exportClip('quick');
    s.applyEvent({ type: 'ExportDone', id: 100, input: FILES[0], output: 'x', bytes: 1 });
    expect('shouldClose' in s).toBe(false);
    expect(s.media?.path).toBe(FILES[0]);
    expect(s.ended).toBe(false);
  });

  it('closing asks only about edits that were not exported', async () => {
    const s = await queued();
    expect(s.unsaved).toBe(false);
    cut(s);
    expect(s.unsaved).toBe(true);
    await s.exportClip('quick');
    expect(s.unsaved).toBe(false);
    s.applyEvent({ type: 'ExportFailed', id: 100, input: FILES[0], message: 'No.', original_intact: true });
    expect(s.unsaved).toBe(true);
  });

  // --- Prev / Next video -------------------------------------------------------------------

  it('the queue is ordered by date and opens on the oldest clip', async () => {
    const s = await queued([FILES[2], FILES[0], FILES[1]]);
    expect(api.orderFiles).toHaveBeenCalledWith([FILES[2], FILES[0], FILES[1]]);
    expect(s.queue.files).toEqual(FILES);
    expect(s.current).toEqual({ path: FILES[0], at: 0 });
    expect(s.media?.path).toBe(FILES[0]);
  });

  it('Next goes newer and Prev older, in the folder', async () => {
    folder();
    const s = await queued([FILES[2]]);
    // Both look ahead on open, from the clip's remembered modified time.
    expect(api.adjacentFile).toHaveBeenCalledWith(FILES[2], modifiedOf(FILES[2]), true);
    expect(api.adjacentFile).toHaveBeenCalledWith(FILES[2], modifiedOf(FILES[2]), false);
    expect(s.canNext).toBe(true);
    expect(s.canPrev).toBe(true);
    expect(s.nextTarget?.path).toBe(D);
    expect(s.prevTarget?.path).toBe(FILES[1]);
    await s.nextVideo();
    expect(api.close).toHaveBeenCalledWith(3);
    expect(s.media?.path).toBe(D);
    await s.prevVideo();
    expect(s.media?.path).toBe(FILES[2]);
    await s.prevVideo();
    expect(s.media?.path).toBe(FILES[1]);
  });

  it('Next walks the queue forward in time, then the folder past the newest; Prev walks it back', async () => {
    folder();
    const s = await queued([FILES[1], FILES[0]]);
    expect(s.current).toEqual({ path: FILES[0], at: 0 });
    await s.nextVideo();
    expect(s.current).toEqual({ path: FILES[1], at: 1 });
    await s.nextVideo();
    expect(s.current).toMatchObject({ path: FILES[2], at: null });
    await s.nextVideo();
    expect(s.media?.path).toBe(D);
    // Back the way it came: through the folder, then into the queue again.
    await s.prevVideo();
    expect(s.media?.path).toBe(FILES[2]);
    await s.prevVideo();
    expect(s.current).toEqual({ path: FILES[1], at: 1 });
    await s.prevVideo();
    expect(s.current).toEqual({ path: FILES[0], at: 0 });
  });

  it('Prev walks the queue back, then the folder past the oldest; Next comes back into the queue', async () => {
    folder();
    const s = await queued([FILES[2], D]);
    // The queue has a step each way it can: no need to ask the folder for Next.
    expect(api.adjacentFile).not.toHaveBeenCalledWith(FILES[2], modifiedOf(FILES[2]), true);
    await s.nextVideo();
    expect(s.current).toEqual({ path: D, at: 1 });
    await s.prevVideo();
    expect(s.current).toEqual({ path: FILES[2], at: 0 });
    await s.prevVideo();
    expect(s.current).toMatchObject({ path: FILES[1], at: null });
    await s.prevVideo();
    expect(s.media?.path).toBe(FILES[0]);
    expect(s.canPrev).toBe(false);
    await s.nextVideo();
    expect(s.media?.path).toBe(FILES[1]);
    await s.nextVideo();
    expect(s.current).toEqual({ path: FILES[2], at: 0 });
    await s.nextVideo();
    expect(s.current).toEqual({ path: D, at: 1 });
  });

  it('each is disabled at its end, and does nothing', async () => {
    folder();
    const newest = await queued([E]);
    expect(newest.canNext).toBe(false);
    expect(newest.canPrev).toBe(true);
    await newest.nextVideo();
    expect(newest.media?.path).toBe(E);

    vi.mocked(api.close).mockClear();
    const oldest = await queued([FILES[0]]);
    expect(oldest.canPrev).toBe(false);
    expect(oldest.canNext).toBe(true);
    await oldest.prevVideo();
    expect(oldest.media?.path).toBe(FILES[0]);
    expect(api.close).not.toHaveBeenCalled();
  });

  it('Next looks again when pressed, and stays put if the folder has nothing after all', async () => {
    folder();
    const s = await queued([FILES[0]]);
    expect(s.canNext).toBe(true);
    vi.mocked(api.adjacentFile).mockResolvedValueOnce(null); // b was deleted in Explorer meanwhile
    await s.nextVideo();
    expect(s.media?.path).toBe(FILES[0]);
    expect(api.close).not.toHaveBeenCalled();
    expect(s.canNext).toBe(false);
  });

  it("Next and Prev say so when the folder can't be read, and stay", async () => {
    folder();
    const s = await queued([FILES[2]]);
    vi.mocked(api.adjacentFile).mockRejectedValueOnce("Couldn't look for the next video.");
    await s.nextVideo();
    expect(s.media?.path).toBe(FILES[2]);
    expect(s.notice).toBe("Couldn't look for the next video.");
    vi.mocked(api.adjacentFile).mockRejectedValueOnce(undefined);
    await s.prevVideo();
    expect(s.media?.path).toBe(FILES[2]);
    expect(s.notice).toBe("Couldn't look for the previous video in this folder.");
  });

  it("a look-ahead that fails keeps its button enabled; pressing it looks again and says why if it still can't", async () => {
    vi.mocked(api.adjacentFile).mockImplementation(async () => {
      throw "Couldn't read this folder.";
    });
    const s = await queued([FILES[2]]);
    await s.exportClip('quick'); // locked: Discard is off, so Prev / Next are the only ways on
    expect(s.canNext).toBe(true);
    expect(s.canPrev).toBe(true);
    await s.prevVideo();
    expect(s.media?.path).toBe(FILES[2]);
    expect(s.notice).toBe("Couldn't read this folder.");
    expect(s.canPrev).toBe(true);
    // Once the folder answers, they go on.
    folder();
    await s.prevVideo();
    expect(s.media?.path).toBe(FILES[1]);
  });

  it('Next on an edited clip that was not exported asks first; Keep editing stays', async () => {
    folder();
    const s = await queued([FILES[0]]);
    cut(s);
    const going = s.nextVideo();
    expect(s.confirm).toBe('move');
    s.answer(false);
    await going;
    expect(s.media?.path).toBe(FILES[0]);
    expect(s.edited).toBe(true);

    const again = s.nextVideo();
    expect(s.confirm).toBe('move');
    s.answer(true);
    await again;
    expect(s.media?.path).toBe(FILES[1]);
  });

  it('Prev asks first only for edits that were not exported', async () => {
    folder();
    const s = await queued([FILES[2]]);
    cut(s);
    const going = s.prevVideo();
    expect(s.confirm).toBe('move');
    s.answer(false);
    await going;
    expect(s.media?.path).toBe(FILES[2]);
    await s.exportClip('audio');
    await s.prevVideo();
    expect(s.confirm).toBeNull();
    expect(s.media?.path).toBe(FILES[1]);
  });

  it('Next after an export does not ask, and leaves the running export alone', async () => {
    folder();
    const s = await queued([FILES[0]]);
    cut(s);
    await s.exportClip('quick');
    await s.nextVideo();
    expect(s.confirm).toBeNull();
    expect(s.media?.path).toBe(FILES[1]);
    expect(s.exports.running).toEqual([100]);
    expect(s.locked).toBe(false);
    // Its progress still lands, for the results list.
    s.applyEvent({ type: 'ExportProgress', id: 100, fraction: 0.3 });
    expect(s.results[0]).toMatchObject({ input: FILES[0], action: 'quick', state: 'running', fraction: 0.3 });
  });

  it('an Export audio of exactly these edits counts as exported; a later edit does not', async () => {
    folder();
    const s = await queued([FILES[0]]);
    cut(s);
    await s.exportClip('audio');
    expect(s.unsaved).toBe(false);
    cut(s, 30);
    expect(s.unsaved).toBe(true);
    const going = s.nextVideo();
    expect(s.confirm).toBe('move');
    s.answer(true);
    await going;
    expect(s.media?.path).toBe(FILES[1]);
  });

  it('the folder walk passes over a clip whose Quick export is still to finish', async () => {
    folder();
    const s = await queued([FILES[2]]);
    await s.nextVideo(); // d, in the folder
    await s.exportClip('quick'); // d goes to the Recycle Bin once it's done
    await s.prevVideo();
    expect(s.current).toEqual({ path: FILES[2], at: 0 });
    expect(s.nextTarget?.path).toBe(E);
    await s.nextVideo(); // the folder after c is d, still there but on its way out: e
    expect(s.media?.path).toBe(E);
  });

  it('the queue walk passes over clips on their way out, or gone', async () => {
    folder();
    const s = await queued(FILES);
    await s.exportClip('quick'); // a: recycled once it's done
    await s.nextVideo();
    folder(FILES[1]); // the folder as it will be once b is recycled
    await s.discard(); // b: recycled now; on to c
    expect(s.current).toEqual({ path: FILES[2], at: 2 });
    // Back past b (gone) and a (on its way out): into the folder, which has nothing older.
    expect(s.canPrev).toBe(false);
    // Once a's export is done it's gone too; once a failed one leaves it, it's back.
    s.applyEvent({ type: 'ExportDone', id: 100, input: FILES[0], output: '[CLIPPED] a.mp4', bytes: 1 });
    expect(s.canPrev).toBe(false);
    await s.addFiles([FILES[0]]); // trimmed again (restored from the Recycle Bin)
    expect(s.queue.files).toEqual([...FILES, FILES[0]]);
    expect(s.canNext).toBe(true);
    await s.nextVideo();
    expect(s.current).toEqual({ path: FILES[0], at: 3 });
  });

  it("a neighbour whose Quick export fails is back: the open clip's look-ahead runs again", async () => {
    folder(E);
    const s = await queued([FILES[2]]);
    await s.nextVideo(); // d, the newest in the folder
    await s.exportClip('quick'); // d is on its way out
    await s.prevVideo();
    expect(s.media?.path).toBe(FILES[2]);
    expect(s.canNext).toBe(false); // d is passed over, and there's nothing newer
    s.applyEvent({ type: 'ExportFailed', id: 100, input: D, message: 'No.', original_intact: true });
    await settle();
    expect(s.canNext).toBe(true);
    expect(s.nextTarget?.path).toBe(D);
  });

  it('a clip whose Quick export failed after recycling it stays passed over', async () => {
    folder();
    const s = await queued(FILES);
    await s.exportClip('quick'); // a: on its way to the Recycle Bin
    await s.nextVideo();
    expect(s.canPrev).toBe(false);
    // Recycled, but the new file then couldn't take its name: the original is no longer there.
    folder(FILES[0]);
    const message = 'Saved the new file as "a.mp4.hasami-tmp", but couldn\'t rename it.';
    s.applyEvent({ type: 'ExportFailed', id: 100, input: FILES[0], message, original_intact: false });
    await settle();
    expect(s.canPrev).toBe(false);
  });

  it("a neighbour kept by a deferred discard is back: the open clip's look-ahead runs again", async () => {
    folder(E);
    const s = await queued([FILES[2]]);
    await s.nextVideo(); // d
    await s.exportClip('audio');
    await s.discard(); // waits for the audio export; nothing newer, so on to c
    expect(s.media?.path).toBe(FILES[2]);
    expect(s.canNext).toBe(false);
    s.applyEvent({ type: 'DiscardKept', path: D, message: 'Kept.' });
    await settle();
    expect(s.canNext).toBe(true);
  });

  it('a clip that cannot be opened can still be moved on from in a queue', async () => {
    vi.mocked(api.open).mockRejectedValueOnce("Couldn't open this clip in the editor.");
    const s = await queued();
    expect(s.error).toBe("Couldn't open this clip in the editor.");
    expect(s.media).toBeNull();
    expect(s.canNext).toBe(true);
    await s.nextVideo();
    expect(s.media?.path).toBe(FILES[1]);
  });

  // --- Discard -----------------------------------------------------------------------------

  it('discard closes the clip, recycles it and moves on through Next', async () => {
    folder();
    const s = await queued([FILES[0]]);
    vi.mocked(api.adjacentFile).mockClear();
    await s.discard();
    expect(s.confirm).toBeNull();
    expect(api.close).toHaveBeenCalledWith(1);
    expect(api.discard).toHaveBeenCalledWith(FILES[0], []);
    // The session lets go of the file before it's recycled.
    expect(vi.mocked(api.close).mock.invocationCallOrder[0]).toBeLessThan(
      vi.mocked(api.discard).mock.invocationCallOrder[0],
    );
    // The recycled clip's remembered modified time still finds the next one.
    expect(api.adjacentFile).toHaveBeenNthCalledWith(1, FILES[0], modifiedOf(FILES[0]), true);
    expect(s.media?.path).toBe(FILES[1]);
  });

  it("discard that can't look for a newer clip moves on to an older one, and says why", async () => {
    folder();
    const s = await queued([FILES[2]]);
    vi.mocked(api.adjacentFile).mockRejectedValueOnce(undefined);
    await s.discard();
    expect(s.media?.path).toBe(FILES[1]);
    expect(s.notice).toBe("Couldn't look for the next video in this folder.");
  });

  it('discard of the newest clip moves on to the next older one', async () => {
    folder();
    const s = await queued([E]);
    await s.discard();
    expect(s.media?.path).toBe(D);
    expect(s.current).toMatchObject({ path: D, at: null });
  });

  it('discard in a queue goes to the next queued file', async () => {
    const s = await queued();
    await s.discard();
    expect(s.current).toEqual({ path: FILES[1], at: 1 });
  });

  it('discard of the last video leaves the window open with nothing to trim', async () => {
    const s = await queued([FILES[0]]);
    await s.discard();
    expect(s.media).toBeNull();
    expect(s.current).toBeNull();
    expect(s.ended).toBe(true);
    expect(s.canNext).toBe(false);
    expect(s.results).toMatchObject([{ input: FILES[0], action: 'discard', state: 'none' }]);
  });

  it('discard waits for the session to close before recycling the file', async () => {
    const s = await queued();
    let closed!: () => void;
    vi.mocked(api.close).mockImplementationOnce(() => new Promise<void>((r) => (closed = r)));
    const discarding = s.discard();
    await Promise.resolve();
    await Promise.resolve();
    expect(api.discard).not.toHaveBeenCalled();
    closed();
    await discarding;
    expect(api.discard).toHaveBeenCalledWith(FILES[0], []);
  });

  it('discard of an edited clip asks first', async () => {
    const s = await queued();
    cut(s);
    const discarding = s.discard();
    expect(s.confirm).toBe('discard');
    s.answer(false);
    await discarding;
    expect(api.discard).not.toHaveBeenCalled();
    expect(s.media?.path).toBe(FILES[0]);
  });

  it('a locked clip cannot be discarded: its export is already dealing with the original', async () => {
    const s = await queued();
    await s.exportClip('quick');
    await s.discard();
    expect(api.discard).not.toHaveBeenCalled();
    expect(s.media?.path).toBe(FILES[0]);
  });

  it('discard after Export audio hands the discard to Rust to do after that export', async () => {
    const s = await queued();
    await s.exportClip('audio');
    await s.discard();
    expect(api.discard).toHaveBeenCalledWith(FILES[0], [100]);
    expect(s.media?.path).toBe(FILES[1]); // moved on at once
    expect(s.results[1]).toMatchObject({ input: FILES[0], action: 'discard', state: 'running' });
    s.applyEvent({ type: 'ExportDone', id: 100, input: FILES[0], output: 'a.mp3', bytes: 1 });
    expect(s.results[1].state).toBe('running'); // the page never recycles by itself
    s.applyEvent({ type: 'DiscardDone', path: FILES[0] });
    expect(s.results[1].state).toBe('none');
    expect(api.discard).toHaveBeenCalledTimes(1);
  });

  it('a discard kept because its export failed shows as kept', async () => {
    const s = await queued(FILES.slice(0, 2));
    await s.exportClip('audio');
    await s.discard();
    s.applyEvent({ type: 'ExportFailed', id: 100, input: FILES[0], message: 'No.', original_intact: true });
    expect(s.results[1].state).toBe('running');
    const message = 'Kept "a.mp4": its export didn\'t finish, so it wasn\'t sent to the Recycle Bin.';
    s.applyEvent({ type: 'DiscardKept', path: FILES[0], message });
    expect(s.results[1]).toMatchObject({ action: 'discard', state: 'failed', message });
    expect(api.discard).toHaveBeenCalledTimes(1);
  });

  it('a discard outcome that arrives before editor_discard answers still lands', async () => {
    const s = await queued(FILES.slice(0, 2));
    await s.exportClip('audio');
    // Rust can resolve the discard (e.g. the export had just failed) and emit before it replies.
    vi.mocked(api.discard).mockImplementationOnce(async (path) => {
      s.applyEvent({ type: 'DiscardKept', path, message: 'Kept.' });
    });
    await s.discard();
    expect(s.results[1]).toMatchObject({ action: 'discard', state: 'failed', message: 'Kept.' });
  });

  it('a deferred discard that Rust refuses leaves no waiting row and reopens the clip', async () => {
    const s = await queued();
    await s.exportClip('audio');
    vi.mocked(api.discard).mockRejectedValueOnce("Couldn't set this clip aside for the Recycle Bin.");
    await s.discard();
    expect(s.log.some((l) => l.action === 'discard')).toBe(false);
    expect(s.media?.path).toBe(FILES[0]);
    expect(s.current).toEqual({ path: FILES[0], at: 0 });
    expect(s.discardFailure).toMatchObject({ message: "Couldn't set this clip aside for the Recycle Bin." });
    // Still the same visit: its Export audio still shows.
    expect(s.audioExport).toMatchObject({ action: 'audio' });
  });

  it('a failed discard reopens the clip and says why under the top bar, not in the status bar', async () => {
    const s = await queued();
    const why = "Couldn't move this clip to the Recycle Bin because another program is using it.";
    vi.mocked(api.discard).mockRejectedValueOnce(why);
    await s.discard();
    expect(s.discardFailure).toMatchObject({ message: why });
    expect(s.notice).toBeNull();
    expect(s.current).toEqual({ path: FILES[0], at: 0 });
    expect(s.media?.path).toBe(FILES[0]);
  });

  it('a failed discard is said again each time, and goes with the clip', async () => {
    const s = await queued();
    vi.mocked(api.discard).mockRejectedValueOnce('No.').mockRejectedValueOnce('No.');
    await s.discard();
    const first = s.discardFailure;
    await s.discard();
    expect(s.discardFailure).toMatchObject({ message: 'No.' });
    expect(s.discardFailure?.seq).not.toBe(first?.seq);
    await s.nextVideo();
    expect(s.discardFailure).toBeNull();
  });

  // --- starting the window -----------------------------------------------------------------

  it('start listens for events before telling Rust it is ready, then opens the oldest file', async () => {
    const order: string[] = [];
    vi.mocked(api.onEvent).mockImplementation(async () => {
      order.push('listen');
      return () => {};
    });
    vi.mocked(api.files).mockImplementation(async () => {
      order.push('files');
      return [FILES[1], FILES[0]];
    });
    vi.mocked(api.ready).mockImplementation(async () => {
      order.push('ready');
    });
    const s = new EditorStore();
    await s.start();
    expect(order).toEqual(['listen', 'files', 'ready']);
    expect(s.current).toEqual({ path: FILES[0], at: 0 });
    expect(s.error).toBeNull();
  });

  it("start says so when it can't get the files to trim", async () => {
    vi.mocked(api.onEvent).mockResolvedValue(() => {});
    vi.mocked(api.files).mockRejectedValue(new Error('ipc down'));
    const logged = vi.spyOn(console, 'error').mockImplementation(() => {});
    const s = new EditorStore();
    await s.start();
    expect(s.error).toMatch(/couldn't start/i);
    expect(api.ready).not.toHaveBeenCalled();
    logged.mockRestore();
  });

  it('a Files event whose merge fails is logged, not left as an unhandled rejection', async () => {
    const s = await queued(FILES.slice(0, 1));
    const logged = vi.spyOn(console, 'error').mockImplementation(() => {});
    vi.spyOn(s, 'addFiles').mockRejectedValueOnce(new Error('boom'));
    s.applyEvent({ type: 'Files', paths: [FILES[1]] });
    await settle();
    expect(logged).toHaveBeenCalled();
    logged.mockRestore();
  });

  // --- more files, and the results list ----------------------------------------------------

  it('Files events merge into the queue, once per path, ordered by date', async () => {
    const s = await queued(FILES.slice(0, 1));
    s.applyEvent({ type: 'Files', paths: [FILES[2], FILES[0], FILES[1], FILES[2]] });
    await settle();
    expect(s.queue.files).toEqual(FILES);
    expect(s.canNext).toBe(true);
  });

  it('a Files event re-sorts only the part of the queue not yet handled', async () => {
    const s = await queued([FILES[2], FILES[0]]);
    expect(s.queue.files).toEqual([FILES[0], FILES[2]]);
    await s.addFiles([E, FILES[1]]);
    // a is open; c, b and e haven't been: they're sorted together.
    expect(api.orderFiles).toHaveBeenLastCalledWith([FILES[2], E, FILES[1]]);
    expect(s.queue.files).toEqual([FILES[0], FILES[1], FILES[2], E]);
    await s.nextVideo();
    await s.nextVideo();
    expect(s.current).toEqual({ path: FILES[2], at: 2 });
    // a, b and c have been opened and keep their places; d goes in with e.
    await s.addFiles([D]);
    expect(api.orderFiles).toHaveBeenLastCalledWith([E, D]);
    expect(s.queue.files).toEqual([...FILES, D, E]);
    await s.nextVideo();
    expect(s.current).toEqual({ path: D, at: 3 });
  });

  it('a Files event whose files Next opens while they are ordered keeps the opened one in place', async () => {
    const s = await queued(FILES.slice(0, 2));
    let release!: () => void;
    vi.mocked(api.orderFiles).mockImplementationOnce(
      (paths) => new Promise((r) => (release = () => r([...paths].sort((x, y) => modifiedOf(x) - modifiedOf(y))))),
    );
    const adding = s.addFiles([FILES[2]]);
    await settle();
    await s.nextVideo(); // b, from the part being ordered
    release();
    await adding;
    expect(s.queue.files).toEqual(FILES);
    expect(s.current).toEqual({ path: FILES[1], at: 1 });
  });

  it('two Files events close together both land', async () => {
    const s = await queued(FILES.slice(0, 1));
    let release!: () => void;
    vi.mocked(api.orderFiles).mockImplementationOnce(
      (paths) => new Promise((r) => (release = () => r([...paths].sort((x, y) => modifiedOf(x) - modifiedOf(y))))),
    );
    const first = s.addFiles([FILES[2]]);
    const second = s.addFiles([FILES[1]]);
    await settle();
    release();
    await Promise.all([first, second]);
    expect(s.queue.files).toEqual(FILES);
  });

  it('files arriving while the folder is being walked come next', async () => {
    folder();
    const s = await queued([FILES[0]]);
    await s.nextVideo();
    expect(s.current).toEqual({ path: FILES[1], at: null, side: 'newer' });
    await s.addFiles([E]);
    expect(s.media?.path).toBe(FILES[1]); // the open clip stays
    await s.nextVideo();
    expect(s.current).toEqual({ path: E, at: 1 });
  });

  it('files arriving with nothing left open open the first of them', async () => {
    const s = await queued([FILES[0]]);
    await s.discard();
    expect(s.ended).toBe(true);
    await s.addFiles([FILES[2]]);
    expect(s.ended).toBe(false);
    expect(s.current).toEqual({ path: FILES[2], at: 1 });
    expect(s.media?.path).toBe(FILES[2]);
  });

  it('a file already handled can be trimmed again: a retry after a failure is appended', async () => {
    folder();
    const s = await queued(FILES.slice(0, 1));
    await s.exportClip('quick');
    await s.nextVideo();
    s.applyEvent({ type: 'ExportFailed', id: 100, input: FILES[0], message: 'No.', original_intact: true });
    await s.addFiles([FILES[0]]);
    expect(s.queue.files).toEqual([FILES[0], FILES[0]]);
    // Still once per path among the files not dealt with yet.
    await s.addFiles([FILES[0], FILES[2], FILES[2]]);
    expect(s.queue.files).toEqual([FILES[0], FILES[0], FILES[2]]);
  });

  it("a file whose Quick / Save raw export is still to finish isn't trimmed again; one exporting audio is", async () => {
    folder();
    const s = await queued(FILES);
    await s.exportClip('audio'); // job 100 only reads a.mp4
    await s.nextVideo();
    await s.exportClip('quick'); // job 101 will recycle b.mp4
    await s.nextVideo();
    await s.exportClip('save_raw'); // job 102 will move c.mp4 to Raw
    await s.addFiles([FILES[1], FILES[2], FILES[0]]);
    expect(s.queue.files).toEqual([...FILES, FILES[0]]);
    // Once the export has ended (here it failed and left the original), a retry is allowed.
    s.applyEvent({ type: 'ExportFailed', id: 101, input: FILES[1], message: 'No.', original_intact: true });
    await s.addFiles([FILES[1], FILES[2]]);
    expect(s.queue.files).toEqual([...FILES, FILES[0], FILES[1]]);
  });

  it('a clip waiting to be recycled is not re-queued by a new trim of it', async () => {
    const s = await queued(FILES.slice(0, 2));
    await s.exportClip('audio');
    await s.discard();
    await s.addFiles([FILES[0]]);
    expect(s.queue.files).toEqual(FILES.slice(0, 2));
    s.applyEvent({ type: 'DiscardKept', path: FILES[0], message: 'Kept.' });
    // Once it's been kept, it can be trimmed again.
    await s.addFiles([FILES[0]]);
    expect(s.queue.files).toEqual([FILES[0], FILES[1], FILES[0]]);
  });

  // --- black border ------------------------------------------------------------------------

  const BORDER = { x: 240, y: 135, w: 1440, h: 810 };
  /** Opens `bordered` with BORDER found around its picture; every other file has none. */
  const withBorder = (...bordered: string[]) =>
    vi.mocked(api.open).mockImplementation(async (path) =>
      media({
        session: sessionOf(path),
        path,
        file_name: path.split('\\').pop()!,
        modified_ms: modifiedOf(path),
        border: bordered.includes(path) ? BORDER : null,
      }),
    );

  it('a video export crops off the border found on open', async () => {
    withBorder(FILES[0]);
    const s = await queued([FILES[0]]);
    expect(s.keepBorder).toBe(false);
    expect(s.crop).toEqual(BORDER);
    await s.exportClip('quick');
    expect(vi.mocked(api.exportClip).mock.calls[0][0].crop).toEqual(BORDER);
  });

  it('Keep border sends no crop, and is not an undo step', async () => {
    withBorder(FILES[0]);
    const s = await queued([FILES[0]]);
    const before = s.history;
    s.keepBorder = true;
    expect(s.history).toBe(before);
    expect(s.edited).toBe(false);
    expect(s.crop).toBeNull();
    await s.exportClip('save_raw');
    expect(vi.mocked(api.exportClip).mock.calls[0][0].crop).toBeNull();
  });

  it('a clip with no border sends no crop', async () => {
    const s = await queued([FILES[0]]);
    expect(s.media?.border).toBeNull();
    expect(s.crop).toBeNull();
    await s.exportClip('quick');
    expect(vi.mocked(api.exportClip).mock.calls[0][0].crop).toBeNull();
  });

  it('Export audio sends no crop, border or not', async () => {
    withBorder(FILES[0]);
    const s = await queued([FILES[0]]);
    await s.exportClip('audio');
    expect(vi.mocked(api.exportClip).mock.calls[0][0].crop).toBeNull();
  });

  it('Keep border goes back off on the next clip', async () => {
    withBorder(FILES[0], FILES[1]);
    const s = await queued();
    s.keepBorder = true;
    await s.nextVideo();
    expect(s.media?.path).toBe(FILES[1]);
    expect(s.keepBorder).toBe(false);
    expect(s.crop).toEqual(BORDER);
  });

  // --- Dim, and an export's note -----------------------------------------------------------

  it("Dim is for listening only: it doesn't touch the timeline, the history or the export", async () => {
    const s = await queued([FILES[0]]);
    cut(s);
    const before = s.history;
    s.dim = true;
    expect(s.history).toBe(before);
    expect(s.timeline).toBe(before.present);
    await s.exportClip('quick');
    const dimmed = vi.mocked(api.exportClip).mock.calls[0][0];
    s.applyEvent({ type: 'ExportFailed', id: 100, input: FILES[0], message: 'No.', original_intact: true });
    s.dim = false;
    await s.exportClip('quick');
    expect(vi.mocked(api.exportClip).mock.calls[1][0]).toEqual(dimmed);
  });

  it('Dim stays on across clips until it is turned off', async () => {
    folder();
    const s = await queued([FILES[0]]);
    s.dim = true;
    await s.nextVideo();
    expect(s.media?.path).toBe(FILES[1]);
    expect(s.dim).toBe(true);
  });

  it("an export's note shows with its result", async () => {
    const s = await queued([FILES[0]]);
    s.fit20 = true;
    await s.exportClip('quick');
    const note = 'Saved "[CLIPPED] a.mp4", but couldn\'t make the 20 MB copy: the disk is full.';
    s.applyEvent({ type: 'ExportDone', id: 100, input: FILES[0], output: '[CLIPPED] a.mp4', bytes: 1, note });
    expect(s.videoExport).toMatchObject({ state: 'done', output: '[CLIPPED] a.mp4', note });
    expect(s.results[0].note).toBe(note);
    await s.exportClip('audio');
    s.applyEvent({ type: 'ExportDone', id: 101, input: FILES[0], output: 'a.mp3', bytes: 1 });
    expect(s.audioExport?.note).toBeUndefined();
  });

  it('the results list has each export and discard in order, with how it stands', async () => {
    folder();
    const s = await queued();
    await s.exportClip('audio');
    await s.exportClip('quick');
    await s.nextVideo();
    await s.nextVideo();
    await s.discard();
    s.applyEvent({ type: 'ExportStarted', id: 100, input: FILES[0] });
    s.applyEvent({ type: 'ExportDone', id: 100, input: FILES[0], output: 'a.mp3', bytes: 2e6 });
    expect(s.results.map((r) => [r.input, r.action, r.state])).toEqual([
      [FILES[0], 'audio', 'done'],
      [FILES[0], 'quick', 'queued'],
      [FILES[2], 'discard', 'none'],
    ]);
    s.applyEvent({ type: 'ExportStarted', id: 101, input: FILES[0] });
    s.applyEvent({ type: 'ExportFailed', id: 101, input: FILES[0], message: 'No.', original_intact: true });
    expect(s.results[1]).toMatchObject({ state: 'failed', message: 'No.', originalIntact: true });
  });
});
