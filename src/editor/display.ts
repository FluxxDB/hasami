// Text and color helpers for the editor's labels. Pure, so they're unit-tested.

import { formatBytes, formatFps } from '../progress/format';
import type { EditorMedia } from './api';
import { MIN_GAIN_DB } from './timeline';

/** Audio track colors, A1 first (spec 9). Track n uses TRACK_COLORS[n % 6]. */
export const TRACK_COLORS = ['#7fa58c', '#c9a063', '#7f9cc4', '#b08bb3', '#4fa8a2', '#d89a3e'];

/** Muted tracks render in grey (spec 5.4): the iron token. */
export const MUTED_COLOR = '#525252';

/** Lane background the clip fills mix over. */
export const LANE_BG = '#0b0b0b';

export function trackColor(n: number): string {
  return TRACK_COLORS[n % TRACK_COLORS.length];
}

/** `a` weighted `wA` over `b`, both `#rrggbb`. */
export function mixHex(a: string, b: string, wA: number): string {
  const ch = (hex: string, i: number) => parseInt(hex.slice(1 + i * 2, 3 + i * 2), 16);
  let out = '#';
  for (let i = 0; i < 3; i++) {
    const v = Math.round(ch(a, i) * wA + ch(b, i) * (1 - wA));
    out += v.toString(16).padStart(2, '0');
  }
  return out;
}

/** "m:ss", or "h:mm:ss" past an hour, to the nearest second. */
export function formatClock(seconds: number): string {
  const total = Math.max(0, Math.round(seconds));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const ss = String(total % 60).padStart(2, '0');
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`;
}

/** "0.0 dB", "−6.0 dB", "+3.5 dB"; the bottom of the range reads "−∞ dB". */
export function formatDb(db: number): string {
  if (db <= MIN_GAIN_DB) return '−∞ dB';
  const text = Math.abs(db).toFixed(1);
  if (text === '0.0') return '0.0 dB';
  return `${db < 0 ? '−' : '+'}${text} dB`;
}

/** The top bar's metadata line: "1920×1080 · 60 fps · HEVC · 2 audio tracks · 1:30 · 412 MB". */
export function metaLine(m: EditorMedia): string {
  const parts: string[] = [];
  if (m.video) {
    parts.push(`${m.video.width}×${m.video.height}`, `${formatFps(m.video.fps)} fps`, m.video.codec.toUpperCase());
  }
  const n = m.tracks.length;
  parts.push(`${n} audio ${n === 1 ? 'track' : 'tracks'}`, formatClock(m.duration_s), formatBytes(m.size_bytes));
  return parts.join(' · ');
}

/** The last part of a Windows or POSIX path: "C:\\clips\\a.mp4" → "a.mp4". */
export function fileName(path: string): string {
  return path.split(/[\\/]/).pop() ?? path;
}
