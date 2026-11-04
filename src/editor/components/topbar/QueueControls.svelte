<script lang="ts">
  // The queue's place ("2 OF 3") and Discard, with the export strip under them: stacked, so the
  // file name keeps its room at the minimum window width. The strip opens the results list
  // (Summary.svelte) and stays once there's anything to list.

  import type { EditorStore } from '../../store.svelte';
  import { RESULTS_ID } from '../Summary.svelte';
  import { stripText } from './topbar';

  let { store }: { store: EditorStore } = $props();

  const results = $derived(store.results);
  const running = $derived(store.exports.running.length);
  const failedCount = $derived(results.filter((r) => r.state === 'failed').length);
  // Exports run one at a time, so the first one still running is the one making progress.
  const fraction = $derived(running > 0 ? (store.exports.progress[store.exports.running[0]] ?? 0) : 0);
  const strip = $derived(stripText(running, failedCount));
  const position = $derived(
    store.queueMode && store.current?.at != null ? `${store.current.at + 1} of ${store.queue.files.length}` : null,
  );
</script>

<div class="side">
  {#if store.currentPath}
    <div class="queue">
      {#if position}<span class="pos cap">{position}</span>{/if}
      <!-- Not while the clip is still opening (the file is being read), nor once a video
           export is dealing with the original. -->
      <button
        type="button"
        class="link cap"
        disabled={store.busy || store.locked || (!store.media && !store.error)}
        onclick={() => store.discard()}>Discard</button
      >
    </div>
  {/if}
  {#if results.length > 0}
    <button type="button" class="strip cap" popovertarget={RESULTS_ID} aria-label="{strip}: show the results">
      <span>{strip}</span>
      {#if running > 0}<span class="rail" aria-hidden="true"><i style:width="{Math.round(fraction * 100)}%"></i></span>{/if}
    </button>
  {/if}
</div>

<style>
  .side {
    flex: none;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 2px;
  }
  .queue {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .pos {
    color: var(--pebble);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
  }

  button {
    flex: none;
    white-space: nowrap;
    color: var(--bone);
    background: transparent;
    border: 0;
    padding: 0;
    transition:
      border-color 0.12s ease-out,
      background-color 0.12s ease-out,
      color 0.12s ease-out,
      text-decoration-color 0.12s ease-out;
  }
  button:disabled {
    opacity: 0.4;
  }

  /* The export strip: a quiet button that opens the results list. */
  .strip {
    display: flex;
    align-items: center;
    gap: 10px;
    padding: 2px 0;
    color: var(--ash);
    font-variant-numeric: tabular-nums;
  }
  .strip > span:first-child {
    text-decoration: underline;
    text-decoration-color: var(--iron);
    text-underline-offset: 4px;
  }
  .strip:hover {
    color: var(--bone);
  }
  .strip:hover > span:first-child {
    text-decoration-color: var(--bone);
  }
  .rail {
    flex: 1;
    min-width: 32px;
    height: 1px;
    background: var(--iron);
  }
  .rail i {
    display: block;
    height: 100%;
    background: var(--bone);
    transition: width 0.2s ease-out;
  }

  .link {
    padding: 6px 4px;
    text-decoration: underline;
    text-decoration-color: var(--iron);
    text-underline-offset: 4px;
  }
  .link:hover:not(:disabled) {
    text-decoration-color: var(--bone);
  }

  @media (max-width: 1440px) {
    .link {
      padding: 6px 2px;
    }
    .queue {
      gap: 8px;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    button,
    .rail i {
      transition: none;
    }
  }
</style>
