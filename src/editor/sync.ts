// Keeps the playback engine and the editor store in step. The store says what to play (timeline,
// shuttle) and where to go (seeks); the engine reports where it is while it plays. A seek is an
// explicit request (`store.seekSeq`), never inferred from the playhead differing from the engine's
// last report, because a seek can land exactly on that report (the end, or 0).
//
// Plain TypeScript, so it's unit-tested; the window's effects just call it when the store changes.

import { STOPPED, type Shuttle } from './playbackModel';
import type { ClipEditor } from './clipEditor.svelte';
import type { Timeline } from './timeline';

/** The part of PlaybackEngine the store drives. */
export interface EngineLike {
  setTimeline(tl: Timeline): void;
  seek(t: number): void;
  setShuttle(s: Shuttle): void;
  onTime(cb: (t: number) => void): () => void;
  onEnded(cb: () => void): () => void;
}

export class EngineSync {
  private forwarded = -1; // the last seekSeq handed to the engine
  private readonly off: (() => void)[];

  constructor(
    private readonly store: ClipEditor,
    private readonly engine: EngineLike,
  ) {
    this.off = [
      engine.onTime((t) => {
        store.playhead = t; // a report, not a seek: seekSeq is untouched
        store.follow();
      }),
      engine.onEnded(() => (store.shuttle = STOPPED)),
    ];
  }

  /** The store's timeline changed. An edit that shortens the video may also have moved the playhead. */
  timeline(tl: Timeline): void {
    this.engine.setTimeline(tl);
    this.seekIfRequested();
  }

  /** Hands the engine any seek it hasn't had yet. */
  seekIfRequested(): void {
    if (this.store.seekSeq === this.forwarded) return;
    this.forwarded = this.store.seekSeq;
    this.engine.seek(this.store.playhead);
  }

  /** The shuttle changed. A seek made with it (Space at the end goes back to 0) goes first. */
  shuttle(s: Shuttle): void {
    this.seekIfRequested();
    this.engine.setShuttle(s);
    if (s.dir === 0) {
      // A stopped playhead sits on a frame, so a cut lands on the frame the viewer shows.
      this.store.seek(this.store.playhead);
      this.seekIfRequested();
    }
  }

  destroy(): void {
    for (const off of this.off) off();
  }
}
