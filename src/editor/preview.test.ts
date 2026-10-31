import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { FIRST_FRAME_TIMEOUT_MS, HEVC_TYPES, isHevc, proxyUpfront, watchFirstFrame, type FrameSource } from './preview';

describe('proxyUpfront', () => {
  // Chromium's real answers: a bare "hvc1" is never playable; full ids are when HEVC decodes.
  const chromium = (hevc: boolean) => (type: string) =>
    /codecs="(hvc1|hev1)\.\d/.test(type) ? (hevc ? 'probably' : '') : type.includes('avc1') ? 'probably' : '';

  it('plays HEVC directly when the webview can decode it, even though a bare "hvc1" says no', () => {
    const canPlay = chromium(true);
    expect(canPlay('video/mp4; codecs="hvc1"')).toBe('');
    expect(proxyUpfront('hevc', canPlay)).toBe(false);
    expect(proxyUpfront('H265', canPlay)).toBe(false);
  });

  it('goes straight to the proxy when no HEVC string is playable', () => {
    expect(proxyUpfront('hevc', chromium(false))).toBe(true);
  });

  it('tries the original when any one HEVC profile is playable (Main only, say)', () => {
    const mainOnly = (type: string) => (type.includes('.1.6.') ? 'maybe' : '');
    expect(proxyUpfront('hevc', mainOnly)).toBe(false);
  });

  it('asks with full codec strings only', () => {
    for (const t of HEVC_TYPES) expect(t).toMatch(/codecs="(hvc1|hev1)\.[12]\.\d\.L\d+\.B0"$/);
  });

  it('never sends other codecs (or no video) to the proxy up front', () => {
    const nothing = () => '';
    expect(proxyUpfront('h264', nothing)).toBe(false);
    expect(proxyUpfront('vp9', nothing)).toBe(false);
    expect(proxyUpfront(undefined, nothing)).toBe(false);
  });

  it('isHevc', () => {
    expect(isHevc('hevc')).toBe(true);
    expect(isHevc('h265')).toBe(true);
    expect(isHevc('h264')).toBe(false);
    expect(isHevc('hevc_amf')).toBe(false);
  });
});

/** A <video> whose frame state is set by hand. */
class FakeVideo implements FrameSource {
  readyState = 0;
  videoWidth = 0;
  private listeners = new Map<string, Set<() => void>>();
  addEventListener(type: string, cb: () => void) {
    if (!this.listeners.has(type)) this.listeners.set(type, new Set());
    this.listeners.get(type)!.add(cb);
  }
  removeEventListener(type: string, cb: () => void) {
    this.listeners.get(type)?.delete(cb);
  }
  emit(type: string) {
    for (const cb of [...(this.listeners.get(type) ?? [])]) cb();
  }
  listenerCount() {
    let n = 0;
    for (const s of this.listeners.values()) n += s.size;
    return n;
  }
  decodeFirstFrame() {
    this.readyState = 2;
    this.videoWidth = 1920;
    this.emit('loadeddata');
  }
}

describe('watchFirstFrame', () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it('stalls when no frame arrives in time', () => {
    const v = new FakeVideo();
    const onStall = vi.fn();
    watchFirstFrame(v, onStall);
    vi.advanceTimersByTime(FIRST_FRAME_TIMEOUT_MS - 1);
    expect(onStall).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    expect(onStall).toHaveBeenCalledTimes(1);
    expect(v.listenerCount()).toBe(0);
  });

  it('stands down once the first frame is decoded', () => {
    const v = new FakeVideo();
    const onStall = vi.fn();
    watchFirstFrame(v, onStall);
    v.decodeFirstFrame();
    expect(v.listenerCount()).toBe(0);
    // A later seek that empties the buffer isn't a stall.
    v.readyState = 1;
    vi.advanceTimersByTime(FIRST_FRAME_TIMEOUT_MS * 2);
    expect(onStall).not.toHaveBeenCalled();
  });

  it('counts metadata with no picture (black frames, no error) as a stall', () => {
    const v = new FakeVideo();
    const onStall = vi.fn();
    watchFirstFrame(v, onStall);
    v.readyState = 4; // data, but no decoded frame size
    v.emit('canplay');
    vi.advanceTimersByTime(FIRST_FRAME_TIMEOUT_MS);
    expect(onStall).toHaveBeenCalledTimes(1);
  });

  it('accepts a frame that is there when the timer fires, even without an event', () => {
    const v = new FakeVideo();
    const onStall = vi.fn();
    watchFirstFrame(v, onStall);
    v.readyState = 2;
    v.videoWidth = 1280;
    vi.advanceTimersByTime(FIRST_FRAME_TIMEOUT_MS);
    expect(onStall).not.toHaveBeenCalled();
  });

  it('does nothing when cancelled (the source changed)', () => {
    const v = new FakeVideo();
    const onStall = vi.fn();
    const stop = watchFirstFrame(v, onStall);
    stop();
    expect(v.listenerCount()).toBe(0);
    vi.advanceTimersByTime(FIRST_FRAME_TIMEOUT_MS * 2);
    expect(onStall).not.toHaveBeenCalled();
  });

  it('does not watch a video that already has a frame', () => {
    const v = new FakeVideo();
    v.readyState = 4;
    v.videoWidth = 1920;
    const onStall = vi.fn();
    watchFirstFrame(v, onStall);
    expect(v.listenerCount()).toBe(0);
    vi.advanceTimersByTime(FIRST_FRAME_TIMEOUT_MS);
    expect(onStall).not.toHaveBeenCalled();
  });
});
