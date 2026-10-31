// Whether the viewer can play a clip's own video, or needs the H.264 proxy (spec 6: play the
// source directly in WebView2's <video> when it can decode it).
//
// Two checks, because neither alone is enough:
// - Before loading, `canPlayType` with full HEVC codec strings. Chromium accepts only complete
//   HEVC ids (`hvc1.<profile>.<compat>.<tier><level>`); a bare "hvc1" is always "", whatever the
//   machine can decode, so asking that way sent every HEVC clip to the proxy.
// - After loading, a watchdog: a source the browser takes but can't decode may never produce a
//   frame and never raise `error`. With no decoded frame in time, the viewer goes to the proxy.

/** HEVC Main and Main 10, in both MP4 sample-entry spellings. Any one playable is enough to try. */
export const HEVC_TYPES = [
  'video/mp4; codecs="hvc1.1.6.L123.B0"',
  'video/mp4; codecs="hev1.1.6.L123.B0"',
  'video/mp4; codecs="hvc1.2.4.L153.B0"',
  'video/mp4; codecs="hev1.2.4.L153.B0"',
];

/** How long the original gets to show its first frame before the viewer gives up on it. */
export const FIRST_FRAME_TIMEOUT_MS = 4000;

/** HTMLMediaElement.HAVE_CURRENT_DATA, spelled out so this runs against test doubles. */
const HAVE_CURRENT_DATA = 2;

export function isHevc(codec: string | undefined): boolean {
  return /^(hevc|h265)$/i.test(codec ?? '');
}

/**
 * True when the clip should go straight to the proxy, without trying the original: it's HEVC and
 * this webview says it can't play any HEVC at all. Every other codec is tried first (a decode
 * error or the first-frame watchdog sends it to the proxy).
 */
export function proxyUpfront(codec: string | undefined, canPlayType: (type: string) => string): boolean {
  return isHevc(codec) && HEVC_TYPES.every((t) => canPlayType(t) === '');
}

/** Just the parts of a <video> the watchdog reads. */
export interface FrameSource {
  readonly readyState: number;
  readonly videoWidth: number;
  addEventListener(type: string, cb: () => void): void;
  removeEventListener(type: string, cb: () => void): void;
}

/** A decoded picture is there: data for the current position, and a known frame size. */
function hasFrame(v: FrameSource): boolean {
  return v.readyState >= HAVE_CURRENT_DATA && v.videoWidth > 0;
}

// Any of these can be the moment the first frame arrives (a seek right after load delays it).
const FRAME_EVENTS = ['loadeddata', 'canplay', 'seeked', 'timeupdate'];

/**
 * Calls `onStall` once if `video` hasn't decoded a frame within `timeoutMs`. Returns a function
 * that stops watching (call it when the source changes). Once a frame has been seen, it stops by
 * itself: a later seek that empties the buffer is not a stall.
 */
export function watchFirstFrame(
  video: FrameSource,
  onStall: () => void,
  timeoutMs = FIRST_FRAME_TIMEOUT_MS,
): () => void {
  let timer: ReturnType<typeof setTimeout> | null = null;
  const stop = () => {
    if (timer !== null) clearTimeout(timer);
    timer = null;
    for (const ev of FRAME_EVENTS) video.removeEventListener(ev, check);
  };
  function check() {
    if (hasFrame(video)) stop();
  }
  if (hasFrame(video)) return () => {};
  for (const ev of FRAME_EVENTS) video.addEventListener(ev, check);
  timer = setTimeout(() => {
    timer = null;
    const stalled = !hasFrame(video);
    stop();
    if (stalled) onStall();
  }, timeoutMs);
  return stop;
}
