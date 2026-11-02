<script lang="ts" module>
  /** The panel's id: the top bar's export strip opens it with `popovertarget`. */
  export const RESULTS_ID = 'results';
</script>

<script lang="ts">
  import { formatBytes, formatPercent } from '../../progress/format';
  import { fileName } from '../display';
  import type { EditorStore, Result } from '../store.svelte';

  // The results list (spec 5.8): each export and discard and how it went, opened from the top
  // bar's export strip. A popover over the editor: it never replaces the editor and never closes
  // the window. Esc, a click outside, or its close button put it away.

  let { store }: { store: EditorStore } = $props();

  const ACTION: Record<Result['action'], string> = {
    quick: 'Quick export',
    save_raw: 'Export + save raw',
    audio: 'Export audio',
    discard: 'Discarded',
  };

  const results = $derived(store.results);
  const running = $derived(store.exports.running.length);
  // Clips, not rows: one clip can have a failed export and a kept discard.
  const failed = $derived(new Set(results.filter((r) => r.state === 'failed').map((r) => r.input)).size);
  const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

  const heading = $derived(
    running > 0
      ? `Finishing ${plural(running, 'export', 'exports')}`
      : failed > 0
        ? `${plural(failed, 'clip', 'clips')} didn't go through`
        : results.length > 0
          ? 'All done'
          : 'Nothing exported yet',
  );

  function detail(r: Result): string {
    if (r.state === 'done' && r.output) return `${fileName(r.output)} · ${formatBytes(r.bytes ?? 0)}`;
    if (r.action === 'discard') return r.state === 'running' ? 'Goes to the Recycle Bin once its export finishes' : 'Sent to the Recycle Bin';
    return '';
  }

  function status(r: Result): string {
    if (r.action === 'discard' && r.state === 'running') return 'Waiting';
    if (r.action === 'discard' && r.state === 'failed') return 'Kept';
    if (r.state === 'queued') return 'Queued';
    if (r.state === 'running') return formatPercent(r.fraction);
    if (r.state === 'done') return 'Done';
    if (r.state === 'failed') return 'Failed';
    return '';
  }
</script>

<div id={RESULTS_ID} class="panel" popover="auto" role="dialog" aria-labelledby="results-heading">
  <header>
    <!-- Polite: "Finishing 2 exports" turning into "All done" is the news here. -->
    <h2 id="results-heading" aria-live="polite">{heading}</h2>
    <button type="button" class="ctl" aria-label="Close the results" popovertarget={RESULTS_ID} popovertargetaction="hide">
      <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="M2 2l8 8M10 2l-8 8" stroke="currentColor" fill="none" /></svg>
    </button>
  </header>
  {#if running > 0}<p class="note">Closing the window is fine: exports keep running.</p>{/if}

  {#if results.length > 0}
    <ol class="results">
      {#each results as r, i (i)}
        <li class:failed={r.state === 'failed'}>
          <div class="what">
            <span class="name" title={r.input}>{fileName(r.input)}</span>
            <span class="action cap">{ACTION[r.action]}</span>
          </div>
          <span class="status cap" class:strong={r.state === 'failed'}>{status(r)}</span>

          {#if r.state === 'running' && r.action !== 'discard'}
            <span class="rail" aria-hidden="true"><i style:width="{Math.round(r.fraction * 100)}%"></i></span>
          {:else if r.state === 'failed'}
            <p class="message">
              {r.message}
              {#if r.originalIntact}<span class="kept">The original was not changed.</span>{/if}
            </p>
          {:else if detail(r)}
            <span class="detail">{detail(r)}</span>
          {/if}
        </li>
      {/each}
    </ol>
  {/if}
</div>

<style>
  .panel {
    /* Hangs from the top bar, at its right, over the viewer. */
    position: fixed;
    inset: 60px 10px auto auto;
    margin: 0;
    width: min(520px, calc(100vw - 20px));
    max-height: calc(100vh - 76px);
    overflow-y: auto;
    padding: 18px 20px 8px;
    background: var(--charcoal);
    color: var(--bone);
    border: 1px solid var(--smoke);
    border-radius: 8px;
    box-shadow: 0 12px 32px rgb(0 0 0 / 0.5);
    font-size: 13px;
    letter-spacing: -0.017em;
    user-select: none;
  }
  .panel:popover-open {
    animation: drop 0.16s cubic-bezier(0.16, 1, 0.3, 1);
  }
  @keyframes drop {
    from {
      transform: translateY(-4px);
    }
  }

  header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 16px;
  }
  h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 500;
    line-height: 1.3;
    letter-spacing: -0.02em;
  }
  .ctl {
    flex: none;
    width: 30px;
    height: 28px;
    margin-right: -8px;
    display: grid;
    place-items: center;
    color: var(--pebble);
    background: transparent;
    border: 0;
    border-radius: 3px;
    transition:
      background-color 0.12s ease-out,
      color 0.12s ease-out;
  }
  .ctl:hover {
    background: var(--smoke);
    color: var(--bone);
  }
  .note {
    margin: 4px 0 0;
    color: var(--pebble);
    line-height: 1.5;
  }

  .results {
    list-style: none;
    margin: 14px 0 0;
    padding: 0;
  }
  li {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    column-gap: 24px;
    row-gap: 6px;
    align-items: baseline;
    padding: 12px 0;
    border-top: 1px solid var(--smoke);
  }
  .what {
    display: flex;
    align-items: baseline;
    gap: 12px;
    min-width: 0;
  }
  .name {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    user-select: text;
  }
  .action {
    flex: none;
    color: var(--pebble);
    white-space: nowrap;
  }
  .status {
    color: var(--pebble);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }
  .status.strong {
    color: var(--bone);
    font-weight: 500;
  }
  .detail,
  .message,
  .rail {
    grid-column: 1 / -1;
  }
  .detail {
    color: var(--pebble);
    font-size: 12px;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
    user-select: text;
  }
  .message {
    margin: 0;
    line-height: 1.5;
    max-width: 65ch;
    user-select: text;
  }
  .kept {
    display: block;
    color: var(--pebble);
  }
  .rail {
    display: block;
    height: 2px;
    margin-top: 2px;
    background: var(--iron);
  }
  .rail i {
    display: block;
    height: 100%;
    background: var(--bone);
    transition: width 0.2s ease-out;
  }

  @media (prefers-reduced-motion: reduce) {
    .panel:popover-open {
      animation: none;
    }
    .rail i,
    .ctl {
      transition: none;
    }
  }
</style>
