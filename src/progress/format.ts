/** Decimal (1000-based) size, e.g. "412 MB", "19.4 MB". */
export function formatBytes(bytes: number): string {
  const units = ['B', 'KB', 'MB', 'GB', 'TB'];
  let value = Math.max(0, bytes);
  let unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1000;
    unit += 1;
  }
  if (unit === 0) return `${Math.round(value)} B`;
  const text = value >= 100 ? value.toFixed(0) : value.toFixed(1).replace(/\.0$/, '');
  return `${text} ${units[unit]}`;
}

/** Frame rate with at most two decimals and no trailing zeros: 29.97002997 → "29.97", 60 → "60". */
export function formatFps(fps: number): string {
  return String(Number(fps.toFixed(2)));
}

/** "m:ss", or "h:mm:ss" past an hour. Rounds up. */
export function formatEta(seconds: number): string {
  const total = Math.max(0, Math.ceil(seconds));
  const h = Math.floor(total / 3600);
  const m = Math.floor((total % 3600) / 60);
  const s = total % 60;
  const ss = String(s).padStart(2, '0');
  return h > 0 ? `${h}:${String(m).padStart(2, '0')}:${ss}` : `${m}:${ss}`;
}

/** Whole percent, floored so it only reads 100% when finished. */
export function formatPercent(fraction: number): string {
  const clamped = Math.min(1, Math.max(0, fraction));
  return `${Math.floor(clamped * 100 + 1e-9)}%`;
}

export interface ProgressInfo {
  pass: number;
  passes: number;
  fraction: number;
  etaS: number | null;
}

/** "Pass 1 of 2 · 38% · about 0:24 left" (pass part only for multi-pass jobs). */
export function progressLine(p: ProgressInfo): string {
  const parts: string[] = [];
  if (p.passes > 1) parts.push(`Pass ${p.pass} of ${p.passes}`);
  parts.push(formatPercent(p.fraction));
  if (p.etaS !== null) parts.push(`about ${formatEta(p.etaS)} left`);
  return parts.join(' · ');
}
