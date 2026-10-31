<script lang="ts">
  import Lanes from './components/Lanes.svelte';
  import StatusBar from './components/StatusBar.svelte';
  import Toolbar from './components/Toolbar.svelte';
  import TopBar from './components/TopBar.svelte';
  import TrackHeaders from './components/TrackHeaders.svelte';
  import Viewer from './components/Viewer.svelte';
  import { fitZoom } from './geometry';
  import type { EditorStore } from './store.svelte';

  interface Props {
    store: EditorStore;
    onMinimize: () => void;
    onToggleMaximize: () => void;
    onClose: () => void;
  }

  let { store, onMinimize, onToggleMaximize, onClose }: Props = $props();

  let lanesWidth = $state(0);
  const FIT_MARGIN_PX = 24; // room after the last frame, so the end edge stays grabbable

  // A newly loaded clip fits the lanes as soon as their width is known.
  $effect(() => {
    if (!store.needsFit || !store.media || lanesWidth <= 0) return;
    store.pxPerSecond = fitZoom(store.media.duration_s, lanesWidth - FIT_MARGIN_PX);
    store.scrollX = 0;
    store.needsFit = false;
  });
</script>

<main class="win">
  <TopBar {store} {onMinimize} {onToggleMaximize} {onClose} />
  <Viewer {store} />
  <Toolbar {store} />
  <div class="timeline">
    <TrackHeaders {store} />
    <Lanes {store} bind:width={lanesWidth} />
  </div>
  <StatusBar {store} />
</main>

<style>
  .win {
    box-sizing: border-box;
    width: 100vw;
    height: 100vh;
    display: flex;
    flex-direction: column;
    background: var(--void);
    border: 1px solid var(--smoke);
    overflow: hidden;
    user-select: none;
    font-size: 13px;
    letter-spacing: -0.017em;
    /* Proportional figures in names; numeric readouts opt into tabular-nums themselves. */
    font-variant-numeric: normal;
    -webkit-font-smoothing: antialiased;
  }
  .win :global(*),
  .win :global(*::before),
  .win :global(*::after) {
    box-sizing: border-box;
  }
  /* Zero-specificity resets, so classes like .cap still set size and tracking. */
  :global(:where(.win) :where(button, input)) {
    font: inherit;
    letter-spacing: inherit;
    color: inherit;
  }
  :global(:where(.win) :where(button)) {
    cursor: pointer;
  }
  :global(:where(.win) :where(button:disabled)) {
    cursor: default;
  }

  .timeline {
    flex: none;
    max-height: 48%;
    min-height: 0;
    display: grid;
    grid-template-columns: 200px minmax(0, 1fr);
    align-items: start;
    overflow-x: hidden;
    overflow-y: auto;
  }
</style>
