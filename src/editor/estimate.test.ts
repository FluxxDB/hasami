import { describe, expect, it } from 'vitest';
import { estimateExportBytes } from './estimate';
import { media } from './fixtures';
import { createTimeline, setMuted } from './timeline';

describe('estimateExportBytes', () => {
  // The fixture: 412 MB over 90 s with 3 source audio tracks; 2 of them have audio.
  const tl = createTimeline(90, 60, [0, 2]);
  const video = (412e6 * 8) / 90 - 3 * 192e3;

  it('is the source video rate × 1.3 for HEVC, plus one AAC track, over the kept length', () => {
    expect(estimateExportBytes(media(), tl)).toBeCloseTo(((video * 1.3 + 192e3) / 8) * 90, 0);
  });

  it('keeps H.264 at the source rate and counts no audio when none is heard', () => {
    const h264 = media({ video: { width: 1920, height: 1080, fps: 60, codec: 'h264' } });
    const silent = setMuted(setMuted(tl, 0, true), 2, true);
    expect(estimateExportBytes(h264, silent)).toBeCloseTo((video / 8) * 90, 0);
  });

  it('never goes below 1 Mbit/s of video', () => {
    const tiny = media({ size_bytes: 2e6, video: { width: 1920, height: 1080, fps: 60, codec: 'h264' } });
    expect(estimateExportBytes(tiny, tl)).toBeCloseTo(((1e6 + 192e3) / 8) * 90, 0);
  });

  it('is 0 with no clip, or a clip of no length', () => {
    expect(estimateExportBytes(null, tl)).toBe(0);
    expect(estimateExportBytes(media({ duration_s: 0 }), tl)).toBe(0);
  });
});
