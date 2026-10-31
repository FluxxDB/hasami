<script lang="ts">
  import { formatClock } from '../display';
  import { zoomFromPos, zoomPos } from '../geometry';
  import type { EditorStore } from '../store.svelte';
  import Slider from './Slider.svelte';
  import { videoEnd } from '../timeline';

  let { store }: { store: EditorStore } = $props();

  const media = $derived(store.media);
  const zoom = $derived(zoomPos(store.pxPerSecond));

  /** The slider zooms around the playhead while it's in view, otherwise around the left edge. */
  function onZoom(pos: number) {
    const px = store.playhead * store.pxPerSecond - store.scrollX;
    const anchor = px >= 0 && px <= store.viewWidth ? px : 0;
    store.zoomAt(zoomFromPos(pos) / store.pxPerSecond, anchor);
  }
</script>

<!-- A group, not role="toolbar": a toolbar promises arrow-key movement between its buttons, and
     here the arrows step frames (spec 5.5), so each tool stays its own Tab stop. -->
<div class="tlbar" role="group" aria-label="Timeline tools">
  <button
    type="button"
    class="tool cap"
    class:on={store.snapping}
    aria-pressed={store.snapping}
    title="Snap (N)"
    onclick={() => store.command('toggleSnap')}
  >
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m6 15-4-4 6.75-6.77a7.79 7.79 0 0 1 11 11L13 22l-4-4 6.39-6.36a2.14 2.14 0 0 0-3-3L6 15" /><path d="m5 8 4 4" /><path d="m12 15 4 4" /></svg>Snap
  </button>
  <button
    type="button"
    class="tool cap"
    class:on={store.linked}
    aria-pressed={store.linked}
    title="Linked selection (Ctrl+Shift+L)"
    onclick={() => store.command('toggleLinked')}
  >
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71" /><path d="M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71" /></svg>Linked
  </button>
  <div class="vsep"></div>
  <button type="button" class="tool cap" title="Cut every track at the playhead (Ctrl+B)" disabled={!media} onclick={() => store.command('cut')}>
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><circle cx="6" cy="6" r="3" /><circle cx="6" cy="18" r="3" /><path d="M20 4 8.12 15.88" /><path d="M14.47 14.48 20 20" /><path d="M8.12 8.12 12 12" /></svg>Cut <span class="k">Ctrl+B</span>
  </button>
  <button type="button" class="tool" aria-label="Undo" title="Undo (Ctrl+Z)" disabled={store.history.past.length === 0} onclick={() => store.command('undo')}>
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M9 14 4 9l5-5" /><path d="M4 9h10.5a5.5 5.5 0 0 1 0 11H11" /></svg>
  </button>
  <button type="button" class="tool" aria-label="Redo" title="Redo (Ctrl+Shift+Z)" disabled={store.history.future.length === 0} onclick={() => store.command('redo')}>
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m15 14 5-5-5-5" /><path d="M20 9H9.5a5.5 5.5 0 0 0 0 11H13" /></svg>
  </button>

  <div class="spacer"></div>

  {#if media}
    <span class="kept cap"><b>{formatClock(videoEnd(store.timeline))}</b> kept of {formatClock(media.duration_s)}</span>
  {/if}
  <div class="zoom">
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" aria-hidden="true"><path d="M5 12h14" /></svg>
    <div class="zoomrail">
      <Slider value={zoom} label="Zoom" valuetext="{Math.round(store.pxPerSecond)} pixels per second" oninput={onZoom} disabled={!media} />
    </div>
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" aria-hidden="true"><path d="M5 12h14M12 5v14" /></svg>
  </div>
</div>

<style>
  .tlbar {
    flex: none;
    height: 42px;
    display: flex;
    align-items: center;
    gap: 4px;
    padding: 0 12px;
    border-top: 1px solid var(--line);
    border-bottom: 1px solid var(--line);
  }
  .tool {
    height: 28px;
    padding: 0 9px;
    display: flex;
    align-items: center;
    gap: 7px;
    border: 0;
    border-radius: 3px;
    color: var(--pebble);
    background: transparent;
    white-space: nowrap;
    transition:
      background-color 0.12s ease-out,
      color 0.12s ease-out;
  }
  .tool:hover:not(:disabled) {
    color: var(--bone);
    background: var(--charcoal);
  }
  .tool.on {
    color: var(--bone);
    background: var(--smoke);
  }
  .tool:disabled {
    opacity: 0.4;
  }
  /* Shortcut hint: one step quieter than the tool's own label. */
  .k {
    color: #8a8787;
  }
  .vsep {
    width: 1px;
    height: 18px;
    margin: 0 8px;
    background: var(--smoke);
  }
  .spacer {
    flex: 1;
  }
  .kept {
    font-variant-numeric: tabular-nums;
    color: var(--pebble);
    margin-right: 14px;
    white-space: nowrap;
  }
  .kept b {
    color: var(--bone);
    font-weight: 500;
  }

  .zoom {
    display: flex;
    align-items: center;
    gap: 8px;
    color: var(--pebble);
  }
  .zoomrail {
    width: 90px;
    display: flex;
  }

  @media (prefers-reduced-motion: reduce) {
    .tool {
      transition: none;
    }
  }
</style>
