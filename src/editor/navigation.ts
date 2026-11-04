// Where Prev and Next go (spec 5.7): through the trim queue first, then on into the folder past
// either end of it. The queue is ordered by date, oldest first: Next goes to the next newer clip
// and Prev to the next older one. Clips on their way out (a Quick / Save raw export or a discard
// still to finish) and clips already gone are passed over.
//
// The store (store.svelte.ts) keeps the queue and asks these where a step leads; the folder is
// read through `adjacentFile` (the `editor_adjacent_file` command), handed in so this runs in tests.

/** Next video goes to the next newer clip, Prev to the next older one. */
export type Direction = 'next' | 'prev';

/**
 * The file that's open (or opening, or failed to open). `at` is its place in the queue, null for
 * one from the folder; `side` says which end of the queue such a folder clip lies past.
 */
export interface Current {
  path: string;
  at: number | null;
  side?: 'older' | 'newer';
}

/**
 * The files Hasami was asked to trim, oldest first. `next` is the first not opened yet; the files
 * from there on are re-sorted when more arrive.
 */
export interface Queue {
  files: string[];
  next: number;
}

/** How many clips on their way out the folder walk passes over before it gives up. */
export const MAX_PASSED = 50;

/**
 * The queued file Prev / Next go to from `from`, passing over the files in `skip` (on their way
 * out, or gone); null when that way leads into the folder. From a queued file the step is to its
 * neighbour. From a folder clip past the newer end, Next goes to the first queued file not opened
 * yet (more arrived meanwhile); every other folder step is the folder's.
 */
export function queueStep(q: Queue, from: Current, dir: Direction, skip: ReadonlySet<string>): Current | null {
  let at: number;
  if (from.at !== null) at = from.at;
  else if (dir === 'next' && from.side === 'newer') at = q.next - 1;
  else return null;
  const by = dir === 'next' ? 1 : -1;
  for (let i = at + by; i >= 0 && i < q.files.length; i += by) {
    const p = q.files[i];
    if (p !== from.path && !skip.has(p)) return { path: p, at: i };
  }
  return null;
}

/** What the folder walk needs to know. */
export interface FolderContext {
  /** The queued files, oldest first. */
  files: readonly string[];
  /** Each opened file's modified time: the walk goes on from it even once the file is gone. */
  modified: ReadonlyMap<string, number>;
  /** Clips on their way out, passed over. */
  leaving: ReadonlySet<string>;
  /** The folder's next newer (or older) video from `path`, whose modified time was `modifiedMs`. */
  adjacentFile(path: string, modifiedMs: number, newer: boolean): Promise<string | null>;
}

/**
 * The folder's next newer (Next) or older (Prev) video from `from`, passing over clips on their
 * way out. One that's queued takes its place in the queue, so walking back from the folder carries
 * on through the queue. Null when there's nothing that way, or nothing known about `from` (it
 * never opened). Rejects when the folder can't be read.
 */
export async function folderStep(from: Current, dir: Direction, ctx: FolderContext): Promise<Current | null> {
  const newer = dir === 'next';
  let path = from.path;
  for (let i = 0; i < MAX_PASSED; i++) {
    const ms = ctx.modified.get(path);
    if (ms === undefined) return null;
    const found = await ctx.adjacentFile(path, ms, newer);
    if (found === null) return null;
    if (!ctx.leaving.has(found)) {
      const at = newer ? ctx.files.indexOf(found) : ctx.files.lastIndexOf(found);
      if (at >= 0) return { path: found, at };
      // Past the queue's newer or older end; a folder clip stays on the side it was on.
      return { path: found, at: null, side: from.at === null ? from.side : newer ? 'newer' : 'older' };
    }
    path = found;
  }
  return null;
}
