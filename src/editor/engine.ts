// Plays the edited timeline: a muted <video> for the picture and one <audio> element per source
// audio track for the sound, kept in sync with it (spec 5.2, 6). Each <audio> goes through a Web
// Audio GainNode so per-track volume and mute apply live, above 0 dB too. Audio is streamed by
// its element, never decoded whole into memory.
//
// The clock is the video while it has a playable source. With no source (an audio-only file, the
// browser mock, a preview that's still being prepared) the clock is the wall clock, so the
// playhead, the transport and the audio still run.

import { STOPPED, audioSourceAt, videoSourceAt, type Shuttle } from './playbackModel';
import { MIN_GAIN_DB, segLen, videoEnd, type Timeline } from './timeline';

/** How far an <audio> element may drift from where it should be before it's re-seeked. */
export const DRIFT_TOLERANCE_S = 0.04;

/** Reverse shuttle steps the picture back this often (and by speed × this). */
const REVERSE_TICK_S = 1 / 30;

const clamp = (x: number, lo: number, hi: number): number => Math.min(Math.max(x, lo), hi);

/** A track's linear gain: 10^(dB/20), or 0 when muted or at the bottom of the fader (−∞). */
export function trackGain(db: number, muted: boolean): number {
  return muted || db <= MIN_GAIN_DB ? 0 : 10 ** (db / 20);
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
  /** Null when the element plays on its own (the fallback): then volume is capped at 0 dB. */
  graph: { node: MediaElementAudioSourceNode; gain: GainNode } | null;
  /** The segment this element is playing, so entering the next one re-seeks it. */
  segId: string | null;
  failed: boolean;
}

export class PlaybackEngine {
  private readonly video: HTMLVideoElement;
  private readonly ctx: AudioContext | null;
  private tl: Timeline | null = null;
  private tracks: Track[] = [];
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

  private timeCbs = new Set<(t: number) => void>();
  private endedCbs = new Set<() => void>();

  constructor(video: HTMLVideoElement, ctx: AudioContext | null) {
    this.video = video;
    this.ctx = ctx;
    video.muted = true;
  }

  /** Points the engine at a clip: the picture's URL ('' for none) and each audio track's URL. */
  load(videoSrc: string, tracks: { index: number; src: string }[]): void {
    if (videoSrc !== this.videoSrc) {
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
    if (prev?.video !== tl.video) this.showFrame(true);
    for (const tr of this.tracks) tr.segId = null; // re-check every track against the new segments
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
    const tr: Track = { index, src, el: null as unknown as HTMLAudioElement, graph: null, segId: null, failed: false };
    this.attach(tr, this.ctx !== null);
    return tr;
  }

  private attach(tr: Track, useGraph: boolean): void {
    const el = new Audio();
    el.preload = 'auto';
    tr.el = el;
    tr.graph = null;
    tr.segId = null;
    if (useGraph && this.ctx) {
      el.crossOrigin = 'anonymous';
      try {
        const node = this.ctx.createMediaElementSource(el);
        const gain = this.ctx.createGain();
        node.connect(gain).connect(this.ctx.destination);
        tr.graph = { node, gain };
      } catch (err) {
        console.warn('Web Audio routing failed; playing the track on its own', err);
        el.removeAttribute('crossorigin');
      }
    }
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
    for (const tr of this.tracks) this.applyGain(tr);
  }

  private applyGain(tr: Track): void {
    const at = this.tl?.audio.find((a) => a.index === tr.index);
    const g = at ? trackGain(at.gainDb, at.muted) : 0;
    if (tr.graph && this.ctx) {
      // A short ramp, so moving the fader doesn't click.
      tr.graph.gain.gain.setTargetAtTime(g, this.ctx.currentTime, 0.015);
    } else {
      tr.el.volume = Math.min(1, g);
      tr.el.muted = g === 0;
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
    const onFrame = (_now: number, meta: VideoFrameCallbackMetadata) => {
      this.vfc = 0;
      if (this.shuttle.dir !== 1 || !this.tl) return;
      // The last frame of a segment starts one frame before its end: once it's up, move on.
      this.crossSegmentEnd(meta.mediaTime, 1.5 / this.tl.fps);
      if (this.shuttle.dir === 1) this.vfc = this.video.requestVideoFrameCallback(onFrame);
    };
    this.vfc = this.video.requestVideoFrameCallback(onFrame);
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
    if (Math.abs(next.src[0] - seg.src[1]) > 0.5 / tl.fps) this.video.currentTime = next.src[0];
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

  /** Each track plays the source time under the playhead, or is paused in a silent gap. */
  private syncAudio(): void {
    const tl = this.tl;
    if (!tl) return;
    const end = videoEnd(tl);
    // While the picture is seeking or buffering, the sound waits for it.
    const waiting = this.videoLive() && (this.video.seeking || this.video.readyState < HTMLMediaElement.HAVE_FUTURE_DATA);
    for (const tr of this.tracks) {
      if (tr.failed) continue;
      const segs = tl.audio.find((a) => a.index === tr.index)?.segs ?? [];
      // Audio past the end of the video is never heard (export drops it too).
      const hit = waiting || this.t >= end ? null : audioSourceAt(segs, this.t);
      const el = tr.el;
      if (!hit) {
        if (!el.paused) el.pause();
        tr.segId = null;
        continue;
      }
      if (el.playbackRate !== this.shuttle.speed) el.playbackRate = this.shuttle.speed;
      const off = Math.abs(el.currentTime - hit.srcT);
      // Entering a segment seeks to its source time; a contiguous one is already there.
      if (tr.segId !== hit.seg.id) {
        tr.segId = hit.seg.id;
        if (off > DRIFT_TOLERANCE_S) el.currentTime = hit.srcT;
      } else if (!el.seeking && off > DRIFT_TOLERANCE_S) {
        el.currentTime = hit.srcT;
      }
      // An element that ran out of file stays quiet until the next seek (play() would rewind it).
      if (el.paused && !el.ended) el.play().catch(() => {});
    }
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
