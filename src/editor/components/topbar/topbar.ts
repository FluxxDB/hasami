// The top bar's words: what each export is called, the row under the bar, what's read out, the
// tooltips. Pure, so it's unit-tested; the components in this folder render it.

import { formatPercent } from '../../../progress/format';
import { fileName } from '../../display';
import type { Result } from '../../exportLog.svelte';
import type { Current, Direction } from '../../navigation';
import type { Crop } from '../../timeline';

/** What each export (and a discard) is called in the bar and the row under it. */
export const LABEL: Record<Result['action'], string> = {
  quick: 'Quick export',
  save_raw: 'Export + save raw',
  audio: 'Export audio',
  discard: 'Discard',
};

/** Said after a failure that left the original where it was. */
export const KEPT = 'The original was not changed.';

/** An export meter's figure: "Queued" until the job queue starts it, then its percentage. */
export const percentText = (r: Result): string => (r.state === 'queued' ? 'Queued' : formatPercent(r.fraction));

/** The tooltip of the border note and the Keep border toggle. */
export function borderTitle(border: Crop, frame: { width: number; height: number }, keep: boolean): string {
  const size = `${frame.width}×${frame.height}`;
  return (
    `A black border was found around the picture: the game fills ${border.w}×${border.h} of the ${size} frame. ` +
    (keep ? 'Video exports keep it.' : `Video exports crop it off and scale the picture back up to ${size}.`)
  );
}

/**
 * The row under the bar: an outcome that needs words. Why the open clip couldn't be discarded, or
 * why an export failed; a saved video export's note (e.g. that its 20 MB copy couldn't be made);
 * or Export audio's saved file. Null when there's nothing to say.
 */
export function outcomeOf(
  path: string,
  discardFailure: { message: string } | null,
  video: Result | null,
  audio: Result | null,
): Result | null {
  if (discardFailure) {
    return {
      input: path,
      action: 'discard',
      state: 'failed',
      fraction: 0,
      message: discardFailure.message,
      originalIntact: true,
    };
  }
  if (video?.state === 'failed' || (video?.state === 'done' && video.note)) return video;
  if (audio?.state === 'failed' || audio?.state === 'done') return audio;
  return null;
}

/** What's read out for an export (once per change: queued, running, saved or failed, not every percent). */
export function announcement(r: Result | null): string {
  if (!r) return '';
  if (r.state === 'done') return r.note ?? `Saved ${fileName(r.output ?? '')}`;
  if (r.state === 'failed') return `${r.message ?? ''} ${r.originalIntact ? KEPT : ''}`;
  return `${LABEL[r.action]} ${r.state === 'queued' ? 'queued' : 'running'}`;
}

const NOTHING: Record<Direction, string> = {
  next: 'No newer video in this folder',
  prev: 'No older video in this folder',
};

/**
 * Prev's or Next's tooltip: where it goes. None while that way is still being checked (or the
 * check failed: pressing it says why), unless there's no clip at all.
 */
export function stepTitle(dir: Direction, to: Current | null | undefined, hasClip: boolean): string | undefined {
  if (to) return `${dir === 'next' ? 'Next' : 'Previous'}: ${fileName(to.path)}`;
  return to === undefined && hasClip ? undefined : NOTHING[dir];
}

/** The export strip's words: how many exports are running, else how many results failed. */
export function stripText(running: number, failed: number): string {
  if (running > 0) return `Exporting ${running}`;
  return failed > 0 ? `Results · ${failed} failed` : 'Results';
}
