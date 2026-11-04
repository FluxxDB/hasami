import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { DRIFT_TOLERANCE_S, PlaybackEngine, masterGain, trackGain, videoTarget } from './engine';
import { createTimeline, cutAt, deleteSegs, setGain, setMaster } from './timeline';

describe('trackGain', () => {
  it('is 10^(dB/20), and 0 when muted or at the bottom of the fader', () => {
    expect(trackGain(0, false)).toBe(1);
    expect(trackGain(-6, false)).toBeCloseTo(0.501, 3);
    expect(trackGain(6, false)).toBeCloseTo(1.995, 3);
    expect(trackGain(-60, false)).toBe(0);
    expect(trackGain(-80, false)).toBe(0);
    expect(trackGain(0, true)).toBe(0);
  });
  it('keeps the drift tolerance at 40 ms', () => {
    expect(DRIFT_TOLERANCE_S).toBe(0.04);
  });
});

describe('masterGain', () => {
  it('is 10^(dB/20), Dim takes 11 dB off, and −∞ is 0', () => {
    expect(masterGain(0, false)).toBe(1);
    expect(masterGain(12, false)).toBeCloseTo(3.981, 3);
    expect(masterGain(0, true)).toBeCloseTo(10 ** (-11 / 20), 9);
    expect(masterGain(6, true)).toBeCloseTo(10 ** (-5 / 20), 9);
    expect(masterGain(-60, false)).toBe(0);
    expect(masterGain(-60, true)).toBe(0);
  });
});

describe('videoTarget', () => {
  // 0–12 then 31–90 of the source (12–31 deleted), at 60 fps.
  const cut = cutAt(cutAt(createTimeline(90, 60, [0]), 12), 31);
  const tl = deleteSegs(cut, [cut.video[1].id], true);

  it('maps timeline time to the segment and source time on screen', () => {
    expect(videoTarget(tl, 5)).toEqual({ index: 0, srcT: 5 });
    expect(videoTarget(tl, 12)).toEqual({ index: 1, srcT: 31 });
    expect(videoTarget(tl, 20)).toEqual({ index: 1, srcT: 39 });
  });
  it('shows the last frame for a playhead parked at the end', () => {
    const t = videoTarget(tl, 71);
    expect(t?.index).toBe(1);
    expect(t?.srcT).toBeCloseTo(90 - 1 / 60, 9);
  });
  it('clamps before the start', () => {
    expect(videoTarget(tl, -1)).toEqual({ index: 0, srcT: 0 });
  });
});

// --- the engine against fake media elements -------------------------------------------------

/** Just enough of an HTMLMediaElement: seeks can be made to take time; frames are delivered by hand. */
class FakeMedia {
  private time = 0;
  seeking = false;
  paused = true;
  ended = false;
  error: MediaError | null = null;
  readyState = 4;
  duration = 90;
  playbackRate = 1;
  volume = 1;
  muted = false;
  preload = '';
  crossOrigin: string | null = null;
  /** When false, a seek stays in flight until `finishSeek()`. */
  instantSeeks = true;
  seekTargets: number[] = [];
  private srcAttr: string | null = null;
  private listeners = new Map<string, Set<() => void>>();
  private frameCbs = new Map<number, VideoFrameRequestCallback>();
  private nextFrameId = 1;

  get currentTime() {
    return this.time;
  }
  set currentTime(t: number) {
    this.time = t;
    this.ended = false;
    this.seekTargets.push(t);
    if (this.instantSeeks) this.emit('seeked');
    else this.seeking = true;
  }
  /** Moves the play position without it counting as a seek (playback running). */
  advanceTo(t: number) {
    this.time = t;
  }
  finishSeek() {
    this.seeking = false;
    this.emit('seeked');
  }
  set src(v: string) {
    this.srcAttr = v;
  }
  get src() {
    return this.srcAttr ?? '';
  }
  getAttribute(name: string) {
    return name === 'src' ? this.srcAttr : null;
  }
  removeAttribute(name: string) {
    if (name === 'src') this.srcAttr = null;
  }
  load() {}
  play() {
    this.paused = false;
    return Promise.resolve();
  }
  pause() {
    this.paused = true;
  }
  addEventListener(type: string, cb: () => void) {
    if (!this.listeners.has(type)) this.listeners.set(type, new Set());
    this.listeners.get(type)!.add(cb);
  }
  removeEventListener(type: string, cb: () => void) {
    this.listeners.get(type)?.delete(cb);
  }
  private emit(type: string) {
    for (const cb of this.listeners.get(type) ?? []) cb();
  }
  requestVideoFrameCallback(cb: VideoFrameRequestCallback) {
    const id = this.nextFrameId++;
    this.frameCbs.set(id, cb);
    return id;
  }
  cancelVideoFrameCallback(id: number) {
    this.frameCbs.delete(id);
  }
  get pendingFrameCallbacks() {
    return this.frameCbs.size;
  }
  /** Delivers the callbacks registered so far for a frame at `mediaTime`. */
  presentFrame(mediaTime: number) {
    const cbs = [...this.frameCbs.values()];
    this.frameCbs.clear();
    for (const cb of cbs) cb(0, { mediaTime } as VideoFrameCallbackMetadata);
  }
}

let now = 0;
let rafQueue: FrameRequestCallback[] = [];

/** Advances the clock to `ms` and runs one animation frame. */
function frameAt(ms: number) {
  now = ms;
  const q = rafQueue;
  rafQueue = [];
  for (const cb of q) cb(ms);
}

beforeEach(() => {
  now = 0;
  rafQueue = [];
  vi.spyOn(performance, 'now').mockImplementation(() => now);
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => rafQueue.push(cb));
  vi.stubGlobal('cancelAnimationFrame', () => {});
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

function makeEngine(audio: FakeMedia[] = []) {
  const video = new FakeMedia();
  const queue = [...audio];
  const engine = new PlaybackEngine(
    video as unknown as HTMLVideoElement,
    null,
    () => (queue.shift() ?? new FakeMedia()) as unknown as HTMLAudioElement,
  );
  return { video, engine };
}

describe('PlaybackEngine: frame callbacks and seeks', () => {
  // 0–12 then 31–90 of the source (12–31 deleted), at 60 fps.
  const cut = cutAt(cutAt(createTimeline(90, 60, [0]), 12), 31);
  const tl = deleteSegs(cut, [cut.video[1].id], true);

  it('jumps the picture over a deleted range at the end of a segment', () => {
    const { video, engine } = makeEngine();
    engine.load('v.mp4', []);
    engine.setTimeline(tl);
    engine.seek(11.5);
    engine.setShuttle({ dir: 1, speed: 1 });
    video.advanceTo(11.99);
    video.presentFrame(11.99);
    expect(video.currentTime).toBe(31);
  });

  it('a frame callback from before a backward seek does not undo the seek', () => {
    const { video, engine } = makeEngine();
    engine.load('v.mp4', []);
    engine.setTimeline(tl);
    engine.seek(19); // source 38, in the second segment
    engine.setShuttle({ dir: 1, speed: 1 });
    video.advanceTo(50);
    expect(video.pendingFrameCallbacks).toBe(1); // registered while the picture was at 50
    engine.seek(5); // back across the cut, into the first segment
    expect(video.currentTime).toBe(5);
    video.presentFrame(50); // the stale frame, shown just before the seek
    expect(video.currentTime).toBe(5);
    // The next real frame is handled as usual.
    video.advanceTo(11.99);
    video.presentFrame(11.99);
    expect(video.currentTime).toBe(31);
  });
});

describe('PlaybackEngine: audio sync', () => {
  // Audio-only clock (no picture), one track covering the whole source.
  const tl = createTimeline(90, 60, [0]);

  it('waits for a slow seek to land and settle before checking drift again', () => {
    const audio = new FakeMedia();
    audio.instantSeeks = false;
    const { engine } = makeEngine([audio]);
    engine.load('', [{ index: 0, src: 'a0.m4a' }]);
    engine.setTimeline(tl);
    engine.seek(10);
    engine.setShuttle({ dir: 1, speed: 1 });
    frameAt(0);
    expect(audio.seekTargets).toEqual([10]); // entering the segment
    frameAt(100); // still seeking
    frameAt(150);
    audio.finishSeek(); // the seek took 150 ms
    frameAt(200); // behind by about the seek's latency, but settling
    frameAt(300);
    expect(audio.seekTargets).toHaveLength(1);
    // Once settled, real drift is corrected, aimed ahead by the seek latency it measured.
    frameAt(800);
    expect(audio.seekTargets).toHaveLength(2);
    expect(audio.seekTargets[1]).toBeCloseTo(10.8 + 0.15, 6);
  });

  it('a slow first seek does not make later seeks overshoot: no sample counts for more than the lead cap', () => {
    const audio = new FakeMedia();
    audio.instantSeeks = false;
    const { engine } = makeEngine([audio]);
    engine.load('', [{ index: 0, src: 'a0.m4a' }]);
    engine.setTimeline(tl);
    engine.seek(10);
    engine.setShuttle({ dir: 1, speed: 1 });
    frameAt(0);
    frameAt(2000);
    audio.finishSeek(); // a cold first seek: 2 s
    frameAt(2500); // drift: re-seeked, aimed ahead by the most a lead may be
    expect(audio.seekTargets).toHaveLength(2);
    expect(audio.seekTargets[1]).toBeCloseTo(12.5 + 0.25, 6);
    frameAt(2550);
    audio.finishSeek(); // a warm seek: 50 ms
    frameAt(3000);
    expect(audio.seekTargets).toHaveLength(3);
    // The average of 0.25 (the 2 s sample, capped) and 0.05, not of 2 and 0.05.
    expect(audio.seekTargets[2]).toBeCloseTo(13 + 0.15, 6);
  });

  it('does not re-seek audio that is within the tolerance', () => {
    const audio = new FakeMedia();
    const { engine } = makeEngine([audio]);
    engine.load('', [{ index: 0, src: 'a0.m4a' }]);
    engine.setTimeline(tl);
    engine.seek(10);
    engine.setShuttle({ dir: 1, speed: 1 });
    frameAt(0);
    for (let ms = 100; ms <= 2000; ms += 100) {
      audio.advanceTo(10 + ms / 1000 - 0.02); // 20 ms behind: fine
      frameAt(ms);
    }
    expect(audio.seekTargets).toEqual([10]);
  });

  it('is silent while shuttling faster than 1x, as in Resolve', () => {
    const audio = new FakeMedia();
    const { engine } = makeEngine([audio]);
    engine.load('', [{ index: 0, src: 'a0.m4a' }]);
    engine.setTimeline(tl);
    engine.seek(10);
    engine.setShuttle({ dir: 1, speed: 2 });
    for (let ms = 0; ms <= 500; ms += 100) frameAt(ms);
    expect(audio.paused).toBe(true);
    expect(audio.seekTargets).toEqual([]);
    engine.setShuttle({ dir: 1, speed: 1 });
    frameAt(600);
    expect(audio.paused).toBe(false);
  });
});

describe('PlaybackEngine: master volume and Dim', () => {
  const tl = createTimeline(90, 60, [0]);

  /** A Web Audio stand-in that records the graph and each gain's target. */
  class FakeGain {
    gain = { value: 1, setTargetAtTime: (v: number) => (this.gain.value = v) };
    to: unknown = null;
    connect<T>(node: T): T {
      this.to = node;
      return node;
    }
    disconnect() {
      this.to = null;
    }
  }
  const fakeCtx = () => {
    const gains: FakeGain[] = [];
    const destination = {};
    const ctx = {
      state: 'running',
      currentTime: 0,
      destination,
      createGain: () => {
        const g = new FakeGain();
        gains.push(g);
        return g;
      },
      createMediaElementSource: () => new FakeGain(),
    };
    return { ctx: ctx as unknown as AudioContext, gains, destination };
  };

  it('routes every track through one master gain before the speakers', () => {
    const { ctx, gains, destination } = fakeCtx();
    const engine = new PlaybackEngine(
      new FakeMedia() as unknown as HTMLVideoElement,
      ctx,
      () => new FakeMedia() as unknown as HTMLAudioElement,
    );
    engine.setTimeline(setMaster(tl, -6));
    engine.load('', [{ index: 0, src: 'a0.m4a' }]);
    const [master, track] = gains;
    expect(master.to).toBe(destination);
    expect(track.to).toBe(master);
    expect(master.gain.value).toBeCloseTo(10 ** (-6 / 20), 9);
    expect(track.gain.value).toBe(1);
    engine.setDim(true);
    expect(master.gain.value).toBeCloseTo(10 ** (-17 / 20), 9);
    engine.setTimeline(setMaster(tl, -60));
    expect(master.gain.value).toBe(0);
  });

  it('without Web Audio, multiplies it into each element volume, capped at 1', () => {
    const el = new FakeMedia();
    const { engine } = makeEngine([el]);
    engine.setTimeline(setMaster(tl, -6));
    engine.load('', [{ index: 0, src: 'a0.m4a' }]);
    expect(el.volume).toBeCloseTo(10 ** (-6 / 20), 9);
    engine.setDim(true);
    expect(el.volume).toBeCloseTo(10 ** (-17 / 20), 9);
    engine.setDim(false);
    engine.setTimeline(setMaster(setGain(tl, 0, 6), 6));
    expect(el.volume).toBe(1);
    engine.setTimeline(setMaster(tl, -60));
    expect(el.volume).toBe(0);
    expect(el.muted).toBe(true);
  });

  it('without Web Audio, Dim still takes 11 dB off a boosted mix: it applies after the cap', () => {
    const el = new FakeMedia();
    const { engine } = makeEngine([el]);
    engine.setTimeline(setMaster(tl, 12));
    engine.load('', [{ index: 0, src: 'a0.m4a' }]);
    expect(el.volume).toBe(1);
    engine.setDim(true);
    expect(el.volume).toBeCloseTo(10 ** (-11 / 20), 9); // ≈ 0.282
    engine.setTimeline(setMaster(tl, 6));
    expect(el.volume).toBeCloseTo(10 ** (-11 / 20), 9);
  });
});
