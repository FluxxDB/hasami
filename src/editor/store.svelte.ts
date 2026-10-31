// The Quick Trim editor's state. Components render from it; edits go through `edit()` so every
// one is an undo step. The timeline itself lives in `history.present`.

import type { EditorMedia, Peaks, Thumb } from './api';
import { initHistory, isEdited, push, type History } from './history';
import { STOPPED, type Shuttle } from './playbackModel';
import { createTimeline, videoEnd, type Timeline } from './timeline';

const DEFAULT_FPS = 30; // for audio-only files, which have no frame rate of their own

export class EditorStore {
  // Raw (not deeply reactive): history snapshots are compared by identity and never mutated.
  media = $state.raw<EditorMedia | null>(null);
  history = $state.raw<History<Timeline>>(initHistory(createTimeline(0, DEFAULT_FPS, [])));
  selection = $state.raw<string[]>([]);
  playhead = $state(0);

  pxPerSecond = $state(20);
  scrollX = $state(0);
  /** Set by `load`; the window fits the clip to the lanes once it knows their width. */
  needsFit = $state(false);

  linked = $state(true);
  snapping = $state(true);
  fit20 = $state(false);
  showEmpty = $state(false);

  peaks = $state.raw<Record<number, Peaks>>({});
  thumbs = $state.raw<Thumb[]>([]);

  shuttle = $state.raw<Shuttle>(STOPPED);
  queue = $state<{ files: string[]; index: number }>({ files: [], index: 0 });
  exports = $state<{
    running: number[];
    done: { input: string; output: string }[];
    failed: { input: string; message: string; originalIntact: boolean }[];
  }>({ running: [], done: [], failed: [] });

  /** A plain-language message when the clip couldn't be opened. */
  error = $state<string | null>(null);

  get timeline(): Timeline {
    return this.history.present;
  }

  get edited(): boolean {
    return isEdited(this.history);
  }

  /** Quick export at source quality: the source's average rate × the kept video length. */
  get estimateBytes(): number {
    const m = this.media;
    if (!m || !(m.duration_s > 0)) return 0;
    return (m.size_bytes / m.duration_s) * videoEnd(this.timeline);
  }

  /** Starts a fresh edit of `media`: one segment per track, nothing selected, no undo. */
  load(media: EditorMedia): void {
    const audio = media.tracks.filter((t) => !t.empty).map((t) => t.index);
    this.media = media;
    this.history = initHistory(createTimeline(media.duration_s, media.video?.fps ?? DEFAULT_FPS, audio));
    this.selection = [];
    this.playhead = 0;
    this.scrollX = 0;
    this.needsFit = true;
    this.peaks = {};
    this.thumbs = [];
    this.shuttle = STOPPED;
    this.error = null;
  }

  /** Records `next` as an undo step (a no-op edit that returns the same timeline records nothing). */
  edit(next: Timeline): void {
    this.history = push(this.history, next);
  }
}
