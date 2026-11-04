// Shared test data for the editor's store tests.

import type { EditorMedia } from './api';

/** A 90 s 1080p60 HEVC clip with three audio tracks, the middle one empty. */
export const media = (over: Partial<EditorMedia> = {}): EditorMedia => ({
  session: 7,
  path: 'C:\\clips\\Replay.mp4',
  file_name: 'Replay.mp4',
  size_bytes: 412e6,
  duration_s: 90,
  bit_rate: null,
  modified_ms: 1_000,
  video: { width: 1920, height: 1080, fps: 60, codec: 'hevc' },
  border: null,
  tracks: [
    { index: 0, empty: false, file: 'a0.m4a' },
    { index: 1, empty: true, file: null },
    { index: 2, empty: false, file: 'a2.m4a' },
  ],
  ...over,
});
