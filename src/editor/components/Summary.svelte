<script lang="ts">
  import { formatBytes, formatPercent } from '../../progress/format';
  import { fileName } from '../display';
  import type { EditorStore, Result } from '../store.svelte';

  // Shown after the last file of a queue while exports are still running or one has failed
  // (spec 5.8). The window closes itself once everything has finished and succeeded.

  interface Props {
    store: EditorStore;
    onMinimize: () => void;
    onClose: () => void;
  }

  let { store, onMinimize, onClose }: Props = $props();

  const ACTION: Record<Result['action'], string> = {
    quick: 'Quick export',
    save_raw: 'Export + save raw',
    audio: 'Export audio',
    skip: 'Skipped',
    discard: 'Discarded',
  };

  const results = $derived(store.results);
  const running = $derived(store.exports.running.length);
  const failed = $derived(results.filter((r) => r.state === 'failed').length);
  const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

  const heading = $derived(
    running > 0
      ? `Finishing ${plural(running, 'export', 'exports')}`
      : failed > 0
        ? `${plural(failed, 'clip', 'clips')} didn't go through`
        : 'All clips done',
  );
  const note = $derived(
    running > 0 && failed === 0
      ? 'This window closes when they finish. Closing it now is fine: they keep running.'
      : running > 0
        ? 'Closing this window is fine: the rest keep running.'
        : null,
  );

  function detail(r: Result): string {
    if (r.state === 'done' && r.output) return `${fileName(r.output)} · ${formatBytes(r.bytes ?? 0)}`;
    if (r.action === 'skip') return 'Left as it was';
    if (r.action === 'discard') return r.state === 'running' ? 'Goes to the Recycle Bin once its export finishes' : 'Sent to the Recycle Bin';
    return '';
  }

  function status(r: Result): string {
    if (r.action === 'discard' && r.state === 'running') return 'Waiting';
    // The job queue runs one export at a time; the rest haven't reported progress yet.
    if (r.state === 'running') return r.fraction > 0 ? formatPercent(r.fraction) : 'Queued';
    if (r.state === 'done') return 'Done';
    if (r.state === 'failed') return 'Failed';
    return '';
  }
</script>

<div class="summary">
  <header class="bar" data-tauri-drag-region>
    <div class="brand"><b>HASAMI</b><em>quick trim</em></div>
    <div class="spacer" data-tauri-drag-region></div>
    <div class="winctl">
      <button type="button" class="ctl" aria-label="Minimize" onclick={onMinimize}>
        <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="M1 6h10" stroke="currentColor" fill="none" /></svg>
      </button>
      <button type="button" class="ctl" aria-label="Close" onclick={onClose}>
        <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"
          ><path d="M2 2l8 8M10 2l-8 8" stroke="currentColor" fill="none" /></svg
        >
      </button>
    </div>
  </header>

  <section class="body" aria-labelledby="summary-heading">
    <div class="column">
      <h1 id="summary-heading">{heading}</h1>
      {#if note}<p class="note">{note}</p>{/if}

      <ol class="results">
        {#each results as r, i (i)}
          <li class:failed={r.state === 'failed'}>
            <div class="what">
              <span class="name" title={r.input}>{fileName(r.input)}</span>
              <span class="action cap">{ACTION[r.action]}</span>
            </div>
            <span class="status cap" class:strong={r.state === 'failed'}>{status(r)}</span>

            {#if r.state === 'running' && r.fraction > 0}
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

      <div class="actions">
        <button type="button" class="btn cap" onclick={onClose}>Close</button>
      </div>
    </div>
  </section>
</div>

<style>
  .summary {
    flex: 1;
    min-height: 0;
    display: flex;
    flex-direction: column;
  }

  .bar {
    flex: none;
    height: 54px;
    display: flex;
    align-items: center;
    padding: 0 10px 0 18px;
    border-bottom: 1px solid var(--line);
  }
  .brand {
    display: flex;
    align-items: baseline;
    gap: 7px;
    white-space: nowrap;
    pointer-events: none;
  }
  .brand b {
    font-weight: 700;
    font-size: 15px;
    letter-spacing: -0.03em;
  }
  .brand em {
    font-family: var(--serif);
    font-size: 17px;
    color: var(--pebble);
    letter-spacing: -0.01em;
  }
  .spacer {
    flex: 1;
    align-self: stretch;
  }
  .winctl {
    display: flex;
    gap: 2px;
  }
  .ctl {
    width: 34px;
    height: 28px;
    display: grid;
    place-items: center;
    color: var(--pebble);
    background: transparent;
    border: 0;
    border-radius: 3px;
  }
  .ctl:hover {
    background: var(--charcoal);
    color: var(--bone);
  }

  .body {
    flex: 1;
    min-height: 0;
    overflow-y: auto;
    padding: 56px 24px 40px;
  }
  .column {
    width: min(640px, 100%);
    margin: 0 auto;
  }
  h1 {
    margin: 0;
    font-size: 24px;
    font-weight: 500;
    line-height: 1.2;
    letter-spacing: -0.025em;
  }
  .note {
    margin: 10px 0 0;
    color: var(--pebble);
    font-size: 13px;
    line-height: 1.5;
  }

  .results {
    list-style: none;
    margin: 32px 0 0;
    padding: 0;
    border-top: 1px solid var(--smoke);
  }
  li {
    display: grid;
    grid-template-columns: minmax(0, 1fr) auto;
    column-gap: 24px;
    row-gap: 6px;
    align-items: baseline;
    padding: 14px 0;
    border-bottom: 1px solid var(--smoke);
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
    font-size: 13px;
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
    height: 1px;
    margin-top: 4px;
    background: var(--iron);
  }
  .rail i {
    display: block;
    height: 100%;
    background: var(--bone);
    transition: width 0.2s ease-out;
  }

  .actions {
    display: flex;
    justify-content: flex-end;
    margin-top: 32px;
  }
  .btn {
    padding: 10px 20px;
    border-radius: 8px;
    border: 1px solid var(--ash);
    background: transparent;
    color: var(--bone);
    transition:
      border-color 0.12s ease-out,
      background-color 0.12s ease-out;
  }
  .btn:hover {
    border-color: var(--bone);
    background: var(--charcoal);
  }
  .btn:active {
    background: var(--smoke);
  }

  @media (prefers-reduced-motion: reduce) {
    .rail i,
    .btn {
      transition: none;
    }
  }
</style>
