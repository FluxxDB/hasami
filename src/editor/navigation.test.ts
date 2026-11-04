import { describe, expect, it } from 'vitest';
import { MAX_PASSED, folderStep, queueStep, type FolderContext } from './navigation';

const V = 'C:\\Videos\\';
const [A, B, C, D, E] = ['a', 'b', 'c', 'd', 'e'].map((n) => `${V}${n}.mp4`);
/** A folder of a…e, oldest first. */
const FOLDER = [A, B, C, D, E];
const none = new Set<string>();

describe('queueStep', () => {
  const q = { files: [A, B, C], next: 3 };

  it('steps to the queued neighbour, passing over the files to skip', () => {
    expect(queueStep(q, { path: A, at: 0 }, 'next', none)).toEqual({ path: B, at: 1 });
    expect(queueStep(q, { path: A, at: 0 }, 'next', new Set([B]))).toEqual({ path: C, at: 2 });
    expect(queueStep(q, { path: C, at: 2 }, 'prev', none)).toEqual({ path: B, at: 1 });
  });

  it('is null past either end of the queue: that way leads into the folder', () => {
    expect(queueStep(q, { path: C, at: 2 }, 'next', none)).toBeNull();
    expect(queueStep(q, { path: A, at: 0 }, 'prev', none)).toBeNull();
  });

  it('from a folder clip past the newer end, Next goes to the first file not opened yet', () => {
    const more = { files: [A, B, C, D], next: 3 };
    expect(queueStep(more, { path: E, at: null, side: 'newer' }, 'next', none)).toEqual({ path: D, at: 3 });
    expect(queueStep(more, { path: E, at: null, side: 'newer' }, 'prev', none)).toBeNull();
  });
});

describe('folderStep', () => {
  const ctx = (over: Partial<FolderContext> = {}): FolderContext => ({
    queuedFiles: () => [B, C],
    modified: new Map(FOLDER.map((p, i) => [p, 1000 + i])),
    leaving: none,
    adjacentFile: async (path, _ms, newer) => {
      const at = FOLDER.indexOf(path);
      return (newer ? FOLDER[at + 1] : FOLDER[at - 1]) ?? null;
    },
    ...over,
  });

  it('finds the folder neighbour, on the side of the queue it lies past', async () => {
    expect(await folderStep({ path: C, at: 1 }, 'next', ctx())).toEqual({ path: D, at: null, side: 'newer' });
    expect(await folderStep({ path: B, at: 0 }, 'prev', ctx())).toEqual({ path: A, at: null, side: 'older' });
    expect(await folderStep({ path: D, at: null, side: 'newer' }, 'next', ctx())).toEqual({
      path: E,
      at: null,
      side: 'newer',
    });
  });

  it('a queued neighbour takes its place in the queue', async () => {
    expect(await folderStep({ path: D, at: null, side: 'newer' }, 'prev', ctx())).toEqual({ path: C, at: 1 });
  });

  it('reads the queue as it is once the folder answers: a file queued meanwhile takes its place', async () => {
    let queued: string[] = [B, C];
    const c = ctx({
      queuedFiles: () => queued,
      adjacentFile: async () => {
        queued = [B, C, D]; // a Files batch lands while the folder is being read
        return D;
      },
    });
    expect(await folderStep({ path: C, at: 1 }, 'next', c)).toEqual({ path: D, at: 2 });
  });

  it('passes over clips on their way out', async () => {
    expect(await folderStep({ path: C, at: 1 }, 'next', ctx({ leaving: new Set([D]) }))).toEqual({
      path: E,
      at: null,
      side: 'newer',
    });
  });

  it('is null with nothing that way, or nothing known about the clip (it never opened)', async () => {
    expect(await folderStep({ path: E, at: null, side: 'newer' }, 'next', ctx())).toBeNull();
    expect(await folderStep({ path: C, at: 1 }, 'next', ctx({ modified: new Map() }))).toBeNull();
  });

  it('gives up after MAX_PASSED clips on their way out', async () => {
    let asked = 0;
    const endless = ctx({
      leaving: { has: () => true } as unknown as ReadonlySet<string>,
      modified: { get: () => 1 } as unknown as ReadonlyMap<string, number>,
      adjacentFile: async () => `${V}${asked++}.mp4`,
    });
    expect(await folderStep({ path: C, at: 1 }, 'next', endless)).toBeNull();
    expect(asked).toBe(MAX_PASSED);
  });
});
