<script lang="ts">
  import { formatBytes } from '../../progress/format';
  import type { EditorStore } from '../store.svelte';
  import { cutCount } from '../timeline';

  let { store }: { store: EditorStore } = $props();

  const HINTS = 'Space play · J/K/L shuttle · ←/→ frame · Del delete · Ctrl+Z undo';

  const tl = $derived(store.timeline);
  const cuts = $derived(cutCount(tl));
  // A linked selection is one selection however many tracks it spans: count distinct links.
  const selectedCount = $derived.by(() => {
    const ids = new Set(store.selection);
    const links = new Set<string>();
    for (const s of [...tl.video, ...tl.audio.flatMap((a) => a.segs)]) if (ids.has(s.id)) links.add(s.link);
    return links.size;
  });
</script>

<footer class="statusbar cap">
  {#if store.notice}
    <!-- An export that couldn't start, or a discard that failed: the clip is still open. -->
    <span class="notice" role="alert">{store.notice}</span>
  {/if}
  {#if store.media}
    <span>{cuts} {cuts === 1 ? 'cut' : 'cuts'} · {selectedCount} selected</span>
    <!-- The size the export buttons will make now is in bone; the other one is for comparison. -->
    <span>
      Quick export ≈ <b class:off={store.fit20}>{formatBytes(store.estimateBytes)}</b> at source quality · Fit to
      20 MB: <b class:off={!store.fit20}>≤ 20 MB</b>
    </span>
  {/if}
  <span class="spacer"></span>
  <!-- Cut short in a narrow window; the tooltip has the whole line. -->
  <span class="hints" title={HINTS}>{HINTS}</span>
</footer>

<style>
  .statusbar {
    flex: none;
    height: 32px;
    display: flex;
    align-items: center;
    gap: 20px;
    padding: 0 16px;
    border-top: 1px solid var(--line);
    color: var(--pebble);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    overflow: hidden;
  }
  b {
    color: var(--bone);
    font-weight: 500;
  }
  b.off {
    color: var(--pebble);
    font-weight: 400;
  }
  .notice {
    flex: none;
    color: var(--bone);
    text-transform: none;
    letter-spacing: normal;
    font-size: 12px;
  }
  .spacer {
    flex: 1;
  }
  .hints {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
