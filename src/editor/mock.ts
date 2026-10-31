// Browser stand-in for the editor's Rust commands, used by api.ts in mock mode
// (http://localhost:5182/editor.html?mock=1). One 90 s 1080p60 clip with three audio tracks,
// the middle one empty, synthetic peaks and flat-colored thumbnails.
//
// `?mock=1` also applies a few edits after load (see `demoEdits`) so the page shows the approved
// mockup's state: cuts, a linked selection, silence on A2 and audio past the end of the video.
// `?mock=fresh` shows the clip exactly as opened. `?mock=proxy` also acts as if the video can't be
// decoded, so the "Preparing preview" step runs (with ProxyProgress events) before it's playable.
//
// `?mock=queue` opens a trim queue of three clips (as opened, no demo edits) whose exports run one
// at a time for a few seconds each, like the job queue; `?mock=queue-fail` makes the second
// clip's exports fail, so the summary stays up (and a Discard waiting on one keeps the clip).
//
// There's no real media behind the mock: the engine plays it on its clock, without picture or sound.
import type { EditorApi, EditorEvent, EditorMedia, Peaks, Thumb } from './api';
import { fileName } from './display';
import type { EditorStore } from './store.svelte';
import { cutAt, deleteSegs, linkedIds, moveEdge } from './timeline';

const PATH = 'C:\\Users\\ledan\\Videos\\Replay 2026-10-09 23-40-17.mp4';
const PARAM = new URLSearchParams(location.search).get('mock') ?? '';
const QUEUE = PARAM.startsWith('queue')
  ? [PATH, 'C:\\Users\\ledan\\Videos\\Replay 2026-10-09 23-52-04.mp4', 'C:\\Users\\ledan\\Videos\\Replay 2026-10-10 00-07-31.mp4']
  : [PATH];
const FAILING = PARAM === 'queue-fail' ? QUEUE[1] : null;
const EXPORT_MS = 4000;
const DURATION = 90;
const PEAK_RATE = 50;
const THUMB_EVERY = 2;

const media: EditorMedia = {
  session: 1,
  path: PATH,
  file_name: 'Replay 2026-10-09 23-40-17.mp4',
  size_bytes: 412_000_000,
  duration_s: DURATION,
  bit_rate: 36_600_000,
  video: { width: 1920, height: 1080, fps: 60, codec: 'hevc' },
  tracks: [
    { index: 0, empty: false, file: 'C:\\Temp\\hasami-editor-1\\a0.m4a' },
    { index: 1, empty: true, file: null },
    { index: 2, empty: false, file: 'C:\\Temp\\hasami-editor-1\\a2.m4a' },
  ],
};

/** Deterministic PRNG, so the mock looks the same on every load. */
function rng(seed: number): () => number {
  let s = seed;
  return () => (s = (s * 16807) % 2147483647) / 2147483647;
}

/** Track 0 sounds like game audio (always busy); track 2 like a mic (mostly quiet, some talk). */
function peaksFor(index: number): Peaks {
  const rnd = rng(7 + index);
  const values: number[] = [];
  const BLOCK = 6; // one loudness per ~0.12 s, so neighbouring waveform bars differ
  let talking = 0;
  for (let b = 0; b < (DURATION * PEAK_RATE) / BLOCK; b++) {
    let level: number;
    if (index === 0) {
      level = 0.3 + 0.65 * Math.abs(Math.sin(b / 9)) * rnd();
    } else {
      if (talking <= 0 && rnd() < 0.08) talking = 3 + Math.floor(rnd() * 10);
      level = talking-- > 0 ? 0.25 + 0.7 * rnd() : 0.04 + 0.08 * rnd();
    }
    for (let i = 0; i < BLOCK; i++) values.push(level * (0.85 + 0.15 * rnd()));
  }
  return { rate: PEAK_RATE, values };
}

// Night-time game scenes: sky, horizon band, ground. Flat fills only.
const SCENES = [
  ['#2b3f6b', '#4a2a3a', '#17121a'],
  ['#3a4f7a', '#20263f', '#3b2a2f'],
  ['#5a3a2a', '#2a1d26', '#121018'],
  ['#24324f', '#3d2b44', '#1a1520'],
  ['#6b4a2b', '#3a2530', '#141018'],
  ['#1f3a4a', '#253045', '#191622'],
];

function thumbUri(i: number, rnd: () => number): string {
  const [sky, band, ground] = SCENES[Math.floor(rnd() * SCENES.length)];
  const h = 44 + Math.round(rnd() * 12);
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 160 90">` +
    `<rect width="160" height="90" fill="${sky}"/>` +
    `<rect y="${h - 10}" width="160" height="12" fill="${band}"/>` +
    `<path d="M0 ${h}L${40 + (i % 5) * 12} ${h - 6}L160 ${h + 4}V90H0Z" fill="${ground}"/>` +
    `<circle cx="${100 + (i % 3) * 14}" cy="24" r="9" fill="#d9a46a" fill-opacity=".45"/>` +
    `</svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}

function thumbs(): Thumb[] {
  const rnd = rng(11);
  const out: Thumb[] = [];
  for (let t = 0, i = 0; t < DURATION; t += THUMB_EVERY, i++) out.push({ t, file: thumbUri(i, rnd) });
  return out;
}

const later = <T>(value: T, ms = 40): Promise<T> => new Promise((r) => setTimeout(() => r(value), ms));

const listeners = new Set<(e: EditorEvent) => void>();

/** A proxy that takes about two seconds, reporting progress the way the Rust side does. */
function proxy(session: number): Promise<string> {
  return new Promise((resolve) => {
    let fraction = 0;
    const timer = setInterval(() => {
      fraction = Math.min(1, fraction + 0.08);
      for (const cb of listeners) cb({ type: 'ProxyProgress', session, fraction });
      if (fraction >= 1) {
        clearInterval(timer);
        resolve('');
      }
    }, 160);
  });
}

let sessions = 0;
let jobs = 0;
/** The job queue runs one export at a time. */
let lastExport: Promise<void> = Promise.resolve();
/** Each export's outcome (true when Done), for discards that wait on them. */
const outcomes = new Map<number, Promise<boolean>>();

function emit(e: EditorEvent): void {
  for (const cb of listeners) cb(e);
}

function runExport(id: number, input: string, mode: string): Promise<boolean> {
  return new Promise((resolve) => {
    emit({ type: 'ExportStarted', id, input });
    let fraction = 0;
    const timer = setInterval(() => {
      fraction = Math.min(1, fraction + 0.05);
      emit({ type: 'ExportProgress', id, fraction });
      if (fraction < 1) return;
      clearInterval(timer);
      const ok = input !== FAILING;
      if (!ok) {
        emit({ type: 'ExportFailed', id, input, message: "Couldn't export this clip: the result didn't check out.", original_intact: true });
      } else {
        const name = fileName(input).replace(/\.[^.]+$/, mode === 'audio' ? '.mp3' : '.mp4');
        const output = input.replace(/[^\\]+$/, `[CLIPPED] ${name}`);
        emit({ type: 'ExportDone', id, input, output, bytes: mode === 'audio' ? 1_400_000 : 18_600_000 });
      }
      resolve(ok);
    }, EXPORT_MS / 20);
  });
}

export const mockApi: EditorApi = {
  files: () => later(QUEUE),
  open: (path) => later({ ...media, session: ++sessions, path, file_name: fileName(path) }),
  peaks: (_session, index) => later(peaksFor(index), 120),
  thumbs: () => later(thumbs(), 80),
  proxy,
  exportClip: (request) => {
    console.info('[mock] editor_export', request);
    const id = ++jobs;
    const outcome = lastExport.then(() => runExport(id, request.input, request.mode));
    outcomes.set(id, outcome);
    lastExport = outcome.then(() => undefined);
    return later(id);
  },
  // Like the Rust side: with `after`, recycle once those exports are Done, else keep the clip.
  discard: (path, after) => {
    console.info('[mock] editor_discard', { path, after });
    if (after.length > 0) {
      void Promise.all(after.map((id) => outcomes.get(id) ?? Promise.resolve(false))).then((oks) =>
        emit(
          oks.every(Boolean)
            ? { type: 'DiscardDone', path }
            : {
                type: 'DiscardKept',
                path,
                message: `Kept "${fileName(path)}": its export didn't finish, so it wasn't sent to the Recycle Bin.`,
              },
        ),
      );
    }
    return later(undefined);
  },
  close: (session) => {
    console.info('[mock] editor_close', { session });
    return later(undefined);
  },
  ready: () => later(undefined, 0),
  onEvent: async (cb) => {
    listeners.add(cb);
    return () => listeners.delete(cb);
  },
  src: (path) => path,
};

/** The approved mockup's state, rebuilt with the real timeline functions. */
export function demoEdits(store: EditorStore): void {
  let tl = store.timeline;
  tl = cutAt(tl, 12);
  tl = cutAt(tl, 31);
  tl = cutAt(tl, 44);
  tl = deleteSegs(tl, [tl.video[3].id], true); // keep 0–44
  const a2 = () => tl.audio[1].segs;
  tl = moveEdge(tl, a2()[1].id, 'start', 19, false); // silence on A2 from 12 to 19
  tl = moveEdge(tl, a2()[2].id, 'end', 52, false); // A2 runs 8 s past the video
  store.edit(tl);
  store.selection = linkedIds(tl, tl.video[1].id);
  store.pxPerSecond = 20;
  store.needsFit = false;
  store.seek(21 + 43 / 60);
}
