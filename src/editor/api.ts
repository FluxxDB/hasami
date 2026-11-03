// TypeScript mirror of the editor's Rust commands and event (src-tauri/src/app.rs, Task 8).
// Field names stay snake_case, exactly as the Rust structs serialise them. `EditorEvent` is serde
// `#[serde(tag = "type")]`, so it's a union discriminated on `type`.
//
// Mock mode (`editor.html?mock=1`, or any page opened outside Tauri) answers every call from
// mock.ts instead, so the window can be built and looked at in a plain browser.
import { convertFileSrc, invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { ExportRequest } from './timeline';

export const EDITOR_EVENT = 'hasami://editor';

export interface EditorMedia {
  session: number;
  path: string;
  file_name: string;
  size_bytes: number;
  duration_s: number;
  bit_rate: number | null;
  /** The file's modified time (ms since the Unix epoch), read on open; Prev / Next find their neighbours from it. */
  modified_ms: number;
  video: { width: number; height: number; fps: number; codec: string } | null;
  tracks: { index: number; empty: boolean; file: string | null }[];
}

export interface Peaks {
  /** Values per second. */
  rate: number;
  /** Each in 0..1. */
  values: number[];
}

export interface Thumb {
  /** Seconds into the source. */
  t: number;
  file: string;
}

export type EditorEvent =
  | { type: 'Files'; paths: string[] }
  | { type: 'ExportStarted'; id: number; input: string }
  | { type: 'ExportProgress'; id: number; fraction: number }
  /** `note`: something to say about a saved export, e.g. that its 20 MB copy couldn't be made. */
  | { type: 'ExportDone'; id: number; input: string; output: string; bytes: number; note?: string }
  | { type: 'ExportFailed'; id: number; input: string; message: string; original_intact: boolean }
  | { type: 'ProxyProgress'; session: number; fraction: number }
  /** A deferred discard (`discard` with `after`) sent the clip to the Recycle Bin. */
  | { type: 'DiscardDone'; path: string }
  /** A deferred discard kept the clip: an export it waited for didn't finish, or recycling failed. */
  | { type: 'DiscardKept'; path: string; message: string };

export interface EditorApi {
  files(): Promise<string[]>;
  open(path: string): Promise<EditorMedia>;
  peaks(session: number, index: number): Promise<Peaks>;
  thumbs(session: number): Promise<Thumb[]>;
  proxy(session: number): Promise<string>;
  exportClip(request: ExportRequest): Promise<number>;
  /**
   * Recycles `path` now when `after` is empty; otherwise the Rust side recycles it once every
   * export in `after` is done (even if this window has closed) and reports with `Discard*`.
   */
  discard(path: string, after: number[]): Promise<void>;
  /**
   * The video in `path`'s folder next to it by date: the next newer one when `newer`, else the
   * next older one (null: none). `path` may be gone already (recycled or moved): `modifiedMs` is
   * its remembered modified time.
   */
  adjacentFile(path: string, modifiedMs: number, newer: boolean): Promise<string | null>;
  /** `paths` oldest first, by modified time then name; unreadable ones last, in their given order. */
  orderFiles(paths: string[]): Promise<string[]>;
  close(session: number): Promise<void>;
  ready(): Promise<void>;
  onEvent(cb: (e: EditorEvent) => void): Promise<() => void>;
  /** A URL the webview can load for a local file (the asset protocol). */
  src(path: string): string;
}

const tauriApi: EditorApi = {
  files: () => invoke('editor_files'),
  open: (path) => invoke('editor_open', { path }),
  peaks: (session, index) => invoke('editor_peaks', { session, index }),
  thumbs: (session) => invoke('editor_thumbs', { session }),
  proxy: (session) => invoke('editor_proxy', { session }),
  exportClip: (request) => invoke('editor_export', { request }),
  discard: (path, after) => invoke('editor_discard', { path, after }),
  // Tauri maps camelCase arguments to the command's snake_case parameters (`modified_ms`).
  adjacentFile: (path, modifiedMs, newer) => invoke('editor_adjacent_file', { path, modifiedMs, newer }),
  orderFiles: (paths) => invoke('editor_order_files', { paths }),
  close: (session) => invoke('editor_close', { session }),
  ready: () => invoke('editor_ready'),
  onEvent: (cb) => listen<EditorEvent>(EDITOR_EVENT, (e) => cb(e.payload)),
  src: (path) => convertFileSrc(path),
};

export const isMock =
  typeof window !== 'undefined' &&
  (new URLSearchParams(window.location.search).has('mock') || !('__TAURI_INTERNALS__' in window));

// The mock is loaded on demand so the shipped bundle never runs it.
const backend: Promise<EditorApi> = isMock ? import('./mock').then((m) => m.mockApi) : Promise.resolve(tauriApi);

export const api: EditorApi = {
  files: () => backend.then((b) => b.files()),
  open: (path) => backend.then((b) => b.open(path)),
  peaks: (session, index) => backend.then((b) => b.peaks(session, index)),
  thumbs: (session) => backend.then((b) => b.thumbs(session)),
  proxy: (session) => backend.then((b) => b.proxy(session)),
  exportClip: (request) => backend.then((b) => b.exportClip(request)),
  discard: (path, after) => backend.then((b) => b.discard(path, after)),
  adjacentFile: (path, modifiedMs, newer) => backend.then((b) => b.adjacentFile(path, modifiedMs, newer)),
  orderFiles: (paths) => backend.then((b) => b.orderFiles(paths)),
  close: (session) => backend.then((b) => b.close(session)),
  ready: () => backend.then((b) => b.ready()),
  onEvent: (cb) => backend.then((b) => b.onEvent(cb)),
  // Mock thumbnails are data: URIs; anything else has no file behind it in a browser.
  src: (path) => (isMock ? (/^(data|blob):/.test(path) ? path : '') : tauriApi.src(path)),
};
