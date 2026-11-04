// The size a Quick export of the open clip will come out at, for the status bar. Pure, so it's
// unit-tested.
//
// How a video export's bitrate is chosen is mirrored from `export_bitrate` and
// `source_video_bitrate` in src-tauri/src/editor/request.rs: keep the two in step.

import type { EditorMedia } from './api';
import { hasAudibleAudio, videoEnd, type Timeline } from './timeline';

/** Source codecs that get more bitrate when re-encoded as H.264 (request.rs `DENSER_CODECS`). */
const DENSER_CODECS = ['hevc', 'av1', 'vp9'];
/** How much more (request.rs `DENSER_CODEC_FACTOR`). */
const DENSER_CODEC_FACTOR = 1.3;
/** Assumed per source audio track (request.rs `AUDIO_TRACK_BPS`); also the export's one AAC track. */
const AUDIO_TRACK_BPS = 192_000;
/** The lowest video bitrate an export uses (request.rs `source_video_bitrate`). */
const MIN_VIDEO_BPS = 1_000_000;

/**
 * Quick export at source quality, × the kept video length: the source's video rate (its average
 * rate less 192 kbit/s per audio track), ×1.3 for HEVC, AV1 and VP9 as the export encodes it,
 * plus one 192 kbit/s AAC track when any audio is heard. A crop doesn't change it: the rate is
 * the source's whatever the picture size. 0 with no clip.
 */
export function estimateExportBytes(media: EditorMedia | null, tl: Timeline): number {
  if (!media || !(media.duration_s > 0)) return 0;
  const sourceBps = (media.size_bytes * 8) / media.duration_s;
  const videoBps = Math.max(MIN_VIDEO_BPS, sourceBps - AUDIO_TRACK_BPS * media.tracks.length);
  const factor = DENSER_CODECS.includes(media.video?.codec ?? '') ? DENSER_CODEC_FACTOR : 1;
  const audioBps = hasAudibleAudio(tl) ? AUDIO_TRACK_BPS : 0;
  return ((videoBps * factor + audioBps) / 8) * videoEnd(tl);
}
