// Plays the edited timeline: a muted <video> for the picture and one <audio> element per source
// audio track for the sound, kept in sync with it (spec 5.2, 6). Each <audio> goes through a Web
// Audio GainNode so per-track volume and mute apply live, above 0 dB too, and then through one
// master GainNode (master volume, and Dim) before the speakers. Audio is streamed by its element,
// never decoded whole into memory.
//
// The clock is the video while it has a playable source. With no source (an audio-only file, the
// browser mock, a preview that's still being prepared) the clock is the wall clock, so the
// playhead, the transport and the audio still run.

import { STOPPED, audioSourceAt, videoSourceAt, type Shuttle } from './playbackModel';
import { MIN_GAIN_DB, isSilent, segLen, videoEnd, type Timeline } from './timeline';

/** How far an <audio> element may drift from where it should be before it's re-seeked. */
export const DRIFT_TOLERANCE_S = 0.04;

/** After an <audio> seek lands, drift isn't checked again for this long, so playback settles. */
const SETTLE_MS = 250;
/** The most an <audio> seek is aimed ahead to make up for its own latency. */
const MAX_SEEK_LEAD_S = 0.25;

/** Reverse shuttle steps the picture back this often (and by speed × this). */
const REVERSE_TICK_S = 1 / 30;

/** HTMLMediaElement.HAVE_FUTURE_DATA, spelled out so the engine runs against test doubles. */
const HAVE_FUTURE_DATA = 3;

const clamp = (x: number, lo: number, hi: number): number => Math.min(Math.max(x, lo), hi);

/** A track's linear gain: 10^(dB/20), or 0 when muted or at the bottom of the fader (−∞). */
export function trackGain(db: number, muted: boolean): number {
  return isSilent(db, muted) ? 0 : 10 ** (db / 20);
}

/** How much Dim takes off everything you hear while editing (spec 5.4). */
export const DIM_DB = 11;

/** The master's linear gain: 10^((master − Dim)/20), or 0 at the bottom of the fader (−∞). */
export function masterGain(masterDb: number, dim: boolean): number {
  return masterDb <= MIN_GAIN_DB ? 0 : 10 ** ((masterDb - (dim ? DIM_DB : 0)) / 20);
}

/**
 * The video segment on screen at timeline time `t` and the source time to show. `videoSourceAt`
 * is half-open, so a playhead parked at the end shows the last frame of the last segment.
 */
export function videoTarget(tl: Timeline, t: number): { index: number; srcT: number } | null {
  if (tl.video.length === 0) return null;
  const hit = videoSourceAt(tl, clamp(t, 0, videoEnd(tl)));
  if (hit) return { index: tl.video.indexOf(hit.seg), srcT: hit.srcT };
  const index = tl.video.length - 1;
  const last = tl.video[index];
  return { index, srcT: Math.max(last.src[0], last.src[1] - 1 / tl.fps) };
}

interface Track {
  index: number;
  src: string;
  el: HTMLAudioElement;
  /**
   * Null when the element plays on its own (the fallback): then its volume stands in for the
   * track's gain times the master's, capped at 0 dB.
   */
  graph: { node: MediaElementAudioSourceNode; gain: GainNode } | null;
  /** The segment this element is playing, so entering the next one re-seeks it. */
  segId: string | null;
  failed: boolean;
  /** When the seek in flight started (performance.now), or null. */
  seekStart: number | null;
  /** How long this element's seeks take, seconds (a running average). */
  latency: number;
  /** Drift isn't checked before this time (performance.now): a seek is in flight or settling. */
  settleUntil: number;
}

export class PlaybackEngine {
  private readonly video: HTMLVideoElement;
  private readonly ctx: AudioContext | null;
  private readonly makeAudio: () => HTMLAudioElement;
  private tl: Timeline | null = null;
  private tracks: Track[] = [];
  /** The mix's last stage before the speakers: master volume and Dim. Null without Web Audio. */
  private readonly master: GainNode | null;
  private dim = false;
  private videoSrc = '';

  /** Timeline time of the picture, seconds. */
  private t = 0;
  /** Index of the video segment on screen. */
  private vi = 0;
  private shuttle: Shuttle = STOPPED;

  private raf = 0;
  private vfc = 0;
  private reverseTimer: ReturnType<typeof setInterval> | 0 = 0;
  private lastWall = 0;
  /**
   * Bumped whenever the engine moves the picture itself (seek, new timeline, new source, segment
   * jump). A frame callback registered before that describes the old position and is ignored.
   */
  private gen = 0;

  private timeCbs = new Set<(t: number) => void>();
  private endedCbs = new Set<() => void>();

  /** `makeAudio` is for tests, which hand in fake elements. */
  constructor(video: HTMLVideoElement, ctx: AudioContext | null, makeAudio: () => HTMLAudioElement = () => new Audio()) {
    this.video = video;
    this.ctx = ctx;
    this.makeAudio = makeAudio;
    video.muted = true;
    this.master = ctx?.createGain() ?? null;
    if (ctx && this.master) this.master.connect(ctx.destination);
  }

  /** Points the engine at a clip: the picture's URL ('' for none) and each audio track's URL. */
  load(videoSrc: string, tracks: { index: number; src: string }[]): void {
    if (videoSrc !== this.videoSrc) {
      this.gen += 1;
      this.videoSrc = videoSrc;
      if (videoSrc) this.video.src = videoSrc;
      else this.video.removeAttribute('src');
      this.video.load(); // also resets playbackRate, which the play loop sets again
    }
    // A track whose file didn't change keeps its element (and its place in the stream).
    const keep = new Map(this.tracks.map((tr) => [`${tr.index}|${tr.src}`, tr]));
    const next: Track[] = [];
    for (const { index, src } of tracks) {
      const key = `${index}|${src}`;
      const old = keep.get(key);
      if (old) {
        keep.delete(key);
        next.push(old);
      } else {
        next.push(this.makeTrack(index, src));
      }
    }
    for (const tr of keep.values()) this.dropElement(tr);
    this.tracks = next;
    this.applyGains();
    this.showFrame(true);
    if (this.shuttle.dir === 1) this.startForward();
  }

  setTimeline(tl: Timeline): void {
    const prev = this.tl;
    this.tl = tl;
    this.applyGains();
    const end = videoEnd(tl);
    if (this.t > end) this.t = end;
    // Only a change to the video track moves the picture; gain and audio edits don't.
    if (prev?.video !== tl.video) {
      this.gen += 1;
      this.showFrame(true);
    }
    for (const tr of this.tracks) tr.segId = null; // re-check every track against the new segments
  }

  /** Dim: everything you hear 11 dB quieter. It's for listening only: nothing in the timeline changes. */
  setDim(on: boolean): void {
    this.dim = on;
    this.applyGains();
  }

  play(): void {
    this.setShuttle({ dir: 1, speed: 1 });
  }

  pause(): void {
    this.setShuttle(STOPPED);
  }

  seek(t: number): void {
    this.t = this.tl ? clamp(t, 0, videoEnd(this.tl)) : Math.max(0, t);
    this.lastWall = performance.now();
    this.gen += 1;
    this.showFrame(true);
    for (const tr of this.tracks) tr.segId = null;
    if (this.shuttle.dir === 1) this.syncAudio();
  }

  setShuttle(s: Shuttle): void {
    const was = this.shuttle;
    this.shuttle = s;
    if (s.dir !== was.dir) this.stopLoops();
    if (s.dir === 0) {
      this.halt();
      return;
    }
    // A browser AudioContext starts suspended until the page has been interacted with.
    if (this.ctx && this.ctx.state === 'suspended') this.ctx.resume().catch(() => {});
    if (s.dir === 1) this.startForward();
    else this.startReverse();
  }

  onTime(cb: (t: number) => void): () => void {
    this.timeCbs.add(cb);
    return () => this.timeCbs.delete(cb);
  }

  onEnded(cb: () => void): () => void {
    this.endedCbs.add(cb);
    return () => this.endedCbs.delete(cb);
  }

  destroy(): void {
    this.shuttle = STOPPED;
    this.stopLoops();
    this.video.pause();
    for (const tr of this.tracks) this.dropElement(tr);
    this.tracks = [];
    this.master?.disconnect();
    this.timeCbs.clear();
    this.endedCbs.clear();
  }

  // --- audio elements ----------------------------------------------------------------------

  /**
   * One <audio> per track, routed element → GainNode → speakers. The asset protocol is a different
   * origin from the page, so the element asks for CORS (`crossOrigin`) or Web Audio would hear
   * silence. If that load fails, or there's no AudioContext, the track falls back to a plain
   * element whose own volume (at most 0 dB) and mute stand in for the gain.
   */
  private makeTrack(index: number, src: string): Track {
    // `el` and `graph` are filled in by attach.
    const tr: Track = {
      index,
      src,
      el: null as unknown as HTMLAudioElement,
      graph: null,
      segId: null,
      failed: false,
      seekStart: null,
      latency: 0,
      settleUntil: 0,
    };
    this.attach(tr, this.ctx !== null);
    return tr;
  }

  private attach(tr: Track, useGraph: boolean): void {
    const el = this.makeAudio();
    el.preload = 'auto';
    tr.el = el;
    tr.graph = null;
    tr.segId = null;
    tr.seekStart = null;
    tr.settleUntil = 0;
    if (useGraph && this.ctx) {
      el.crossOrigin = 'anonymous';
      try {
        const node = this.ctx.createMediaElementSource(el);
        const gain = this.ctx.createGain();
        // Start at the track's level: a ramp from the default 1.0 would blip a muted track.
        gain.gain.value = this.gainFor(tr);
        node.connect(gain).connect(this.master ?? this.ctx.destination);
        tr.graph = { node, gain };
      } catch (err) {
        console.warn('Web Audio routing failed; playing the track on its own', err);
        el.removeAttribute('crossorigin');
      }
    }
    el.addEventListener('seeked', () => {
      if (tr.el !== el) return;
      const t = performance.now();
      if (tr.seekStart !== null) {
        const took = (t - tr.seekStart) / 1000;
        tr.latency = tr.latency ? (tr.latency + took) / 2 : took;
        tr.seekStart = null;
      }
      tr.settleUntil = t + SETTLE_MS;
    });
    el.addEventListener('error', () => {
      if (tr.el !== el) return;
      if (tr.graph) {
        // Most likely the CORS request was refused: retry as a plain element.
        console.warn(`Audio track ${tr.index} failed through Web Audio; retrying without it`, el.error);
        this.dropElement(tr);
        this.attach(tr, false);
        this.applyGain(tr);
        if (this.shuttle.dir === 1) this.syncAudio();
      } else {
        console.error(`Audio track ${tr.index} could not be played`, el.error);
        tr.failed = true;
      }
    });
    el.src = tr.src;
  }

  private dropElement(tr: Track): void {
    tr.el.pause();
    tr.graph?.node.disconnect();
    tr.graph?.gain.disconnect();
    tr.graph = null;
    tr.el.removeAttribute('src');
    tr.el.load();
  }

  private applyGains(): void {
    if (this.master && this.ctx) this.master.gain.setTargetAtTime(this.masterLevel(), this.ctx.currentTime, 0.015);
    for (const tr of this.tracks) this.applyGain(tr);
  }

  private masterLevel(): number {
    return masterGain(this.tl?.masterDb ?? 0, this.dim);
  }

  private gainFor(tr: Track): number {
    const at = this.tl?.audio.find((a) => a.index === tr.index);
    return at ? trackGain(at.gainDb, at.muted) : 0;
  }

  private applyGain(tr: Track): void {
    const g = this.gainFor(tr);
    if (tr.graph && this.ctx) {
      // A short ramp, so moving the fader doesn't click.
      tr.graph.gain.gain.setTargetAtTime(g, this.ctx.currentTime, 0.015);
    } else {
      // A plain element skips the master node, so the master is applied here.
      const v = g * this.masterLevel();
      tr.el.volume = Math.min(1, v);
      tr.el.muted = v === 0;
    }
  }

  // --- picture -----------------------------------------------------------------------------

  /** True while the <video> has a source to play; otherwise the wall clock drives playback. */
  private videoLive(): boolean {
    return this.videoSrc !== '' && this.video.error === null;
  }

  /** Puts the frame for `this.t` on screen (unless it's already within half a frame of it). */
  private showFrame(force = false): void {
    const tl = this.tl;
    if (!tl) return;
    const target = videoTarget(tl, this.t);
    if (!target) return;
    this.vi = target.index;
    if (!this.videoLive()) return;
    if (!force && this.video.seeking) return;
    if (Math.abs(this.video.currentTime - target.srcT) > 0.5 / tl.fps) this.video.currentTime = target.srcT;
  }

  private stopLoops(): void {
    cancelAnimationFrame(this.raf);
    this.raf = 0;
    if (this.vfc) this.video.cancelVideoFrameCallback?.(this.vfc);
    this.vfc = 0;
    if (this.reverseTimer) clearInterval(this.reverseTimer);
    this.reverseTimer = 0;
  }

  /** Stops everything where it is. */
  private halt(): void {
    this.stopLoops();
    this.video.pause();
    for (const tr of this.tracks) {
      tr.el.pause();
      tr.segId = null;
    }
    this.showFrame(true);
  }

  /** Stops at `t` (the end, or 0 in reverse) and says so. */
  private finish(t: number): void {
    this.shuttle = STOPPED;
    this.t = t;
    this.halt();
    this.emitTime();
    for (const cb of this.endedCbs) cb();
  }

  private emitTime(): void {
    for (const cb of this.timeCbs) cb(this.t);
  }

  // --- forward -----------------------------------------------------------------------------

  private startForward(): void {
    const tl = this.tl;
    if (!tl) return;
    if (this.t >= videoEnd(tl) - 0.5 / tl.fps) {
      this.finish(videoEnd(tl));
      return;
    }
    this.showFrame(true);
    this.lastWall = performance.now();
    if (this.videoLive()) {
      this.video.playbackRate = this.shuttle.speed;
      this.video.play().catch(() => {}); // a failed play shows up as a video error
      this.watchFrames();
    }
    if (!this.raf) this.raf = requestAnimationFrame(this.tick);
  }

  /** Segment ends are caught on the frame the video presents (requestVideoFrameCallback). */
  private watchFrames(): void {
    if (this.vfc || !('requestVideoFrameCallback' in this.video)) return;
    const gen = this.gen;
    this.vfc = this.video.requestVideoFrameCallback((_now, meta) => {
      this.vfc = 0;
      if (this.shuttle.dir !== 1 || !this.tl) return;
      // A frame from before the engine last moved the picture says nothing about where it is now.
      // `currentTime` reflects a seek at once, so the smaller of the two is never a stale frame's.
      if (gen === this.gen && !this.video.seeking) {
        // The last frame of a segment starts one frame before its end: once it's up, move on.
        this.crossSegmentEnd(Math.min(meta.mediaTime, this.video.currentTime), 1.5 / this.tl.fps);
      }
      if (this.shuttle.dir === 1) this.watchFrames();
    });
  }

  /**
   * At the end of the video segment on screen, jumps the <video> to the next segment's start
   * (no jump when the next one carries straight on from it), or stops at the end of the video.
   */
  private crossSegmentEnd(mediaTime: number, early: number): void {
    const tl = this.tl!;
    const seg = tl.video[this.vi];
    if (!seg || mediaTime < seg.src[1] - early) return;
    const next = tl.video[this.vi + 1];
    if (!next) {
      this.finish(videoEnd(tl));
      return;
    }
    this.vi += 1;
    this.t = next.at;
    if (Math.abs(next.src[0] - seg.src[1]) > 0.5 / tl.fps) {
      this.gen += 1;
      this.video.currentTime = next.src[0];
    }
    for (const tr of this.tracks) tr.segId = null;
  }

  private tick = (): void => {
    this.raf = 0;
    const tl = this.tl;
    if (this.shuttle.dir !== 1 || !tl) return;
    const now = performance.now();
    const dt = (now - this.lastWall) / 1000;
    this.lastWall = now;
    const end = videoEnd(tl);

    if (this.videoLive()) {
      if (this.video.playbackRate !== this.shuttle.speed) this.video.playbackRate = this.shuttle.speed;
      // A backstop for a missed frame callback: the true end of the segment. A file that ends a
      // little before its reported duration counts as reaching it (play() would rewind it to 0).
      this.crossSegmentEnd(this.video.ended ? Infinity : this.video.currentTime, 0);
      if (this.shuttle.dir !== 1) return; // finished
      if (this.video.paused && !this.video.seeking && !this.video.ended) this.video.play().catch(() => {});
      this.watchFrames();
      const seg = tl.video[this.vi];
      if (seg) this.t = seg.at + clamp(this.video.currentTime - seg.src[0], 0, segLen(seg));
    } else {
      this.t = Math.min(end, this.t + dt * this.shuttle.speed);
      if (this.t >= end) {
        this.finish(end);
        return;
      }
      this.vi = videoTarget(tl, this.t)?.index ?? 0;
    }

    this.syncAudio();
    this.emitTime();
    this.raf = requestAnimationFrame(this.tick);
  };

  /**
   * Each track plays the source time under the playhead, or is paused in a silent gap. Above 1×
   * the sound is off, as in Resolve's fast shuttle (and no seek could keep up with it anyway).
   */
  private syncAudio(): void {
    const tl = this.tl;
    if (!tl) return;
    const end = videoEnd(tl);
    const speed = this.shuttle.speed;
    // While the picture is seeking or buffering, the sound waits for it (the last half second of
    // the file has no "future data", which isn't buffering).
    const v = this.video;
    const buffering = v.readyState < HAVE_FUTURE_DATA && !(v.duration - v.currentTime < 0.5);
    const waiting = this.videoLive() && (v.seeking || buffering);
    const silent = waiting || speed > 1 || this.t >= end; // past the video end: dropped on export too
    const tolerance = DRIFT_TOLERANCE_S * Math.max(1, speed);
    const now = performance.now();
    for (const tr of this.tracks) {
      if (tr.failed) continue;
      const segs = tl.audio.find((a) => a.index === tr.index)?.segs ?? [];
      const hit = silent ? null : audioSourceAt(segs, this.t);
      const el = tr.el;
      if (!hit) {
        if (!el.paused) el.pause();
        tr.segId = null;
        continue;
      }
      if (el.playbackRate !== speed) el.playbackRate = speed;
      const off = Math.abs(el.currentTime - hit.srcT);
      if (tr.segId !== hit.seg.id) {
        // Entering a segment seeks to its source time; a contiguous one is already there.
        tr.segId = hit.seg.id;
        if (off > tolerance) this.seekAudio(tr, hit.srcT);
      } else if (!el.seeking && now >= tr.settleUntil && off > tolerance) {
        // Drift is only judged once the last seek has landed and settled.
        this.seekAudio(tr, hit.srcT);
      }
      // An element that ran out of file stays quiet until the next seek (play() would rewind it).
      if (el.paused && !el.ended) el.play().catch(() => {});
    }
  }

  /** Seeks a track, aimed ahead by how long its seeks take, so it lands in sync rather than behind. */
  private seekAudio(tr: Track, srcT: number): void {
    const lead = Math.min(MAX_SEEK_LEAD_S, tr.latency) * this.shuttle.speed;
    tr.seekStart = performance.now();
    tr.settleUntil = Infinity; // until it lands ('seeked')
    tr.el.currentTime = srcT + lead;
  }

  // --- reverse -----------------------------------------------------------------------------

  /** Reverse shuttle: sound off, the picture stepped back by speed/30 s every 1/30 s. */
  private startReverse(): void {
    this.video.pause();
    for (const tr of this.tracks) {
      tr.el.pause();
      tr.segId = null;
    }
    if (this.reverseTimer) return;
    this.reverseTimer = setInterval(() => {
      if (this.shuttle.dir !== -1 || !this.tl) return;
      this.t = Math.max(0, this.t - this.shuttle.speed * REVERSE_TICK_S);
      if (this.t <= 0) {
        this.finish(0);
        return;
      }
      // Let a slow seek finish rather than restarting it every tick; the clock keeps going.
      this.showFrame(false);
      this.emitTime();
    }, REVERSE_TICK_S * 1000);
  }
}
