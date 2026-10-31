<script lang="ts">
  import { formatBytes } from '../../progress/format';
  import type { EditorStore } from '../store.svelte';
  import { cutCount } from '../timeline';

  let { store }: { store: EditorStore } = $props();

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
  {#if store.media}
    <span>{cuts} {cuts === 1 ? 'cut' : 'cuts'} · {selectedCount} selected</span>
    <span>
      Quick export ≈ <b>{formatBytes(store.estimateBytes)}</b> at source quality · Fit to 20 MB: <b>≤ 20 MB</b>
    </span>
  {/if}
  <span class="spacer"></span>
  <span class="hints">Space play · J/K/L shuttle · ←/→ frame · Del delete · Ctrl+Z undo</span>
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
  .spacer {
    flex: 1;
  }
  .hints {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
</style>
