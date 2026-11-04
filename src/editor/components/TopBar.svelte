<script lang="ts">
  // The editor's top bar, which is also the window's title bar and drag region: the wordmark, the
  // open clip (topbar/FileTitle), the queue and Discard (topbar/QueueControls), the export
  // controls (topbar/ExportControls), Prev / Next (topbar/StepButtons) and the window buttons;
  // and under it, the row that says how an export or a discard went (topbar/OutcomeRow).

  import { inFlight } from '../exportLog.svelte';
  import type { EditorStore } from '../store.svelte';
  import ExportControls from './topbar/ExportControls.svelte';
  import FileTitle from './topbar/FileTitle.svelte';
  import OutcomeRow from './topbar/OutcomeRow.svelte';
  import QueueControls from './topbar/QueueControls.svelte';
  import StepButtons from './topbar/StepButtons.svelte';
  import { announcement, borderTitle, outcomeOf } from './topbar/topbar';
  import WindowControls from './topbar/WindowControls.svelte';

  interface Props {
    store: EditorStore;
    onMinimize: () => void;
    onToggleMaximize: () => void;
    onClose: () => void;
  }

  let { store, onMinimize, onToggleMaximize, onClose }: Props = $props();

  const media = $derived(store.media);
  const video = $derived(store.videoExport);
  const audio = $derived(store.audioExport);
  const outcome = $derived(media ? outcomeOf(store.currentPath ?? '', store.discardFailure, video, audio) : null);
  /** Read out once per change: a video export in the bar, else an outcome, else a running Export audio. */
  const announce = $derived(announcement(media && store.locked ? video : (outcome ?? (inFlight(audio) ? audio : null))));
  // A black border found on open (spec 5.7): the meta line says it's cropped off (or kept), and
  // Keep border sits under the 20 MB copy toggle, with the other export settings.
  const title = $derived(
    store.border && media?.video ? borderTitle(store.border, media.video, store.keepBorder) : undefined,
  );
</script>

<header class="topbar" data-tauri-drag-region>
  <div class="brand"><b>HASAMI</b><em>quick trim</em></div>
  <div class="vsep brand-sep"></div>
  <FileTitle {store} borderTitle={title} />
  <div class="spacer" data-tauri-drag-region></div>

  {#if store.currentPath || store.results.length > 0}
    <QueueControls {store} />
    <div class="vsep"></div>
  {/if}

  <span class="sr-only" role="status">{announce}</span>

  <ExportControls {store} borderTitle={title} />

  <div class="vsep"></div>
  <StepButtons {store} />
  <WindowControls {onMinimize} {onToggleMaximize} {onClose} />
</header>

<OutcomeRow {store} {outcome} />

<style>
  .topbar {
    flex: none;
    height: 54px;
    display: flex;
    align-items: center;
    gap: 16px;
    padding: 0 10px 0 18px;
    border-bottom: 1px solid var(--line);
  }
  /* Text in the bar lets clicks through to the drag region underneath. */
  .brand {
    pointer-events: none;
    display: flex;
    align-items: baseline;
    gap: 7px;
    white-space: nowrap;
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
  .vsep {
    flex: none;
    width: 1px;
    height: 22px;
    background: var(--smoke);
  }
  .spacer {
    flex: 1;
    align-self: stretch;
  }

  /* Discard, the export strip and Next video share the bar with the export buttons: below a wide
     window, tighten it so the file name keeps some room. */
  @media (max-width: 1440px) {
    .topbar {
      gap: 10px;
      padding-left: 14px;
    }
  }

  /* Then the file name outranks the wordmark's serif half. */
  @media (max-width: 1360px) {
    .brand em {
      display: none;
    }
  }

  /* Near the 960px minimum it needs the wordmark's room too (the title bar and taskbar still say
     Hasami). */
  @media (max-width: 1100px) {
    .brand,
    .vsep {
      display: none;
    }
    .topbar {
      gap: 8px;
    }
  }
</style>
