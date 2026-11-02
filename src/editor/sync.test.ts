import { describe, expect, it } from 'vitest';
import type { EditorMedia } from './api';
import { STOPPED, type Shuttle } from './playbackModel';
import { EditorStore } from './store.svelte';
import { EngineSync, type EngineLike } from './sync';
import { videoEnd, type Timeline } from './timeline';

const media: EditorMedia = {
  session: 1,
  path: 'C:\\clips\\a.mp4',
  file_name: 'a.mp4',
  size_bytes: 1e6,
  duration_s: 90,
  bit_rate: null,
  modified_ms: 0,
  video: { width: 1920, height: 1080, fps: 60, codec: 'h264' },
  tracks: [{ index: 0, empty: false, file: 'a0.m4a' }],
};

/** Records what it's asked to do, and reports like the real engine. */
class FakeEngine implements EngineLike {
  t = 0;
  tl: Timeline | null = null;
  shuttle: Shuttle = STOPPED;
  seeks: number[] = [];
  private timeCb: ((t: number) => void) | null = null;
  private endedCb: (() => void) | null = null;

  setTimeline(tl: Timeline) {
    this.tl = tl;
  }
  seek(t: number) {
    this.t = t;
    this.seeks.push(t);
  }
  setShuttle(s: Shuttle) {
    this.shuttle = s;
  }
  onTime(cb: (t: number) => void) {
    this.timeCb = cb;
    return () => (this.timeCb = null);
  }
  onEnded(cb: () => void) {
    this.endedCb = cb;
    return () => (this.endedCb = null);
  }
  /** A time report while playing. */
  report(t: number) {
    this.t = t;
    this.timeCb?.(t);
  }
  /** What the real engine does when playback runs into `t` (the end, or 0 in reverse). */
  finishAt(t: number) {
    this.shuttle = STOPPED;
    this.report(t);
    this.endedCb?.();
  }
}

/**
 * Stands in for the window's three effects: each calls the sync when its piece of the store
 * changed. `shuttleFirst` runs them in the other order, which must give the same result.
 */
function effects(store: EditorStore, sync: EngineSync, shuttleFirst: boolean) {
  let tl: Timeline | null = null;
  let seq = -1;
  let shuttle: Shuttle | null = null;
  const runTimeline = () => {
    if (store.timeline !== tl) sync.timeline((tl = store.timeline));
  };
  const runSeek = () => {
    if (store.seekSeq !== seq) {
      seq = store.seekSeq;
      sync.seekIfRequested();
    }
  };
  const runShuttle = () => {
    if (store.shuttle !== shuttle) sync.shuttle((shuttle = store.shuttle));
  };
  return () => {
    // Like effects, settle until nothing changes (stopping the shuttle seeks, for one).
    for (let i = 0; i < 3; i++) {
      runTimeline();
      if (shuttleFirst) {
        runShuttle();
        runSeek();
      } else {
        runSeek();
        runShuttle();
      }
    }
  };
}

function setup(shuttleFirst = false) {
  const store = new EditorStore();
  store.load(media);
  const engine = new FakeEngine();
  const sync = new EngineSync(store, engine);
  const flush = effects(store, sync, shuttleFirst);
  flush();
  return { store, engine, sync, flush };
}

describe('EngineSync', () => {
  it('forwards the timeline, seeks and the shuttle', () => {
    const { store, engine, flush } = setup();
    expect(engine.tl).toBe(store.timeline);
    store.seek(10);
    flush();
    expect(engine.t).toBe(10);
    store.command('L');
    flush();
    expect(engine.shuttle).toEqual({ dir: 1, speed: 1 });
  });

  for (const shuttleFirst of [false, true]) {
    const order = shuttleFirst ? ' (shuttle effect first)' : '';

    it(`after playing out to the end, Home then End shows the end again${order}`, () => {
      const { store, engine, flush } = setup(shuttleFirst);
      const end = videoEnd(store.timeline);
      store.command('playPause');
      flush();
      engine.finishAt(end);
      flush();
      expect(store.shuttle.dir).toBe(0);
      store.command('home');
      flush();
      expect(engine.t).toBe(0);
      store.command('end');
      flush();
      expect(engine.t).toBe(end); // the bug: the engine stayed at 0
    });

    it(`after reversing to the start, End then Space plays from the start${order}`, () => {
      const { store, engine, flush } = setup(shuttleFirst);
      const end = videoEnd(store.timeline);
      store.seek(5);
      store.command('J');
      flush();
      engine.finishAt(0);
      flush();
      store.command('end');
      flush();
      expect(engine.t).toBe(end);
      store.command('playPause'); // from the end: back to 0, then play
      flush();
      expect(engine.t).toBe(0); // the bug: the engine stayed at the end and finished at once
      expect(engine.shuttle).toEqual({ dir: 1, speed: 1 });
    });
  }

  it("never sends the engine's own time reports back to it as seeks", () => {
    const { store, engine, flush } = setup();
    store.command('L');
    flush();
    const before = engine.seeks.length;
    for (let t = 1; t < 10; t++) {
      engine.report(t);
      flush();
    }
    expect(store.playhead).toBe(9);
    expect(engine.seeks.length).toBe(before);
  });

  it('stopping snaps the playhead to a frame and shows that frame', () => {
    const { store, engine, flush } = setup();
    store.command('L');
    flush();
    engine.report(12.3456);
    store.command('K');
    flush();
    expect(store.playhead).toBe(Math.round(12.3456 * 60) / 60);
    expect(engine.t).toBe(store.playhead);
  });

  it('stops listening to the engine when destroyed', () => {
    const { store, engine, sync } = setup();
    sync.destroy();
    engine.report(42);
    expect(store.playhead).toBe(0);
  });
});
