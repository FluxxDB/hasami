<script lang="ts">
  // The open clip's name and its meta line, with the Edited dot and the border note; while the
  // next clip opens (or when it couldn't be opened), its name and that it's opening.

  import { fileName, metaLine } from '../../display';
  import type { EditorStore } from '../../store.svelte';

  let { store, borderTitle }: { store: EditorStore; borderTitle?: string } = $props();

  const media = $derived(store.media);
  const path = $derived(store.currentPath);
  /** Said while the next clip opens, or when it couldn't be opened; empty once it's open. */
  const opening = $derived(media || !path ? '' : store.error ? "Couldn't open" : 'Opening…');
</script>

<div class="file">
  {#if media}
    <!-- The name and meta line take the pointer for their tooltips (they're cut short in a
         narrow window) and are drag regions themselves, so the window still moves from them. -->
    <div class="name">
      <h1 class="fname" title={media.file_name} data-tauri-drag-region>{media.file_name}</h1>
      {#if store.edited}<span class="dot" aria-hidden="true"></span><span class="cap edited">Edited</span>{/if}
    </div>
    <div class="metarow">
      {#if store.border}<span class="note cap" title={borderTitle} data-tauri-drag-region
          >{store.keepBorder ? 'Border kept' : 'Border removed'}</span
        >{/if}
      <div class="meta cap" title={metaLine(media)} data-tauri-drag-region>{metaLine(media)}</div>
    </div>
  {:else if path}
    <!-- Opening the next clip, or it couldn't be opened (the viewer says why). -->
    <div class="name"><h1 class="fname" title={fileName(path)} data-tauri-drag-region>{fileName(path)}</h1></div>
    <div class="meta cap">{opening}</div>
  {/if}
  <!-- Always mounted, so a change of its text ("Opening…", "Couldn't open") is announced. -->
  <span class="sr-only" role="status">{opening}</span>
</div>

<style>
  /* Text in the bar lets clicks through to the drag region underneath. */
  .file {
    pointer-events: none;
    display: flex;
    flex-direction: column;
    gap: 4px;
    min-width: 0;
    flex: 0 1 auto;
    overflow: hidden;
  }
  .name {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    white-space: nowrap;
    line-height: 1.25;
  }
  .fname {
    margin: 0;
    font: inherit;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
  }
  .fname,
  .meta[title] {
    pointer-events: auto;
  }
  .dot {
    width: 7px;
    height: 7px;
    border-radius: 50%;
    background: var(--red);
    flex: none;
  }
  .edited {
    color: var(--pebble);
    flex: none;
  }
  .metarow {
    display: flex;
    align-items: baseline;
    min-width: 0;
    line-height: 1.25;
  }
  /* The border note leads the meta line, a step brighter: it says the picture is cropped. */
  .note {
    flex: none;
    color: var(--ash);
    white-space: nowrap;
    pointer-events: auto;
  }
  /* The separator goes with the meta line, so it's cut off with it in a narrow window. */
  .note + .meta::before {
    content: '·';
    margin: 0 0.5em;
  }
  .meta {
    color: var(--pebble);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    line-height: 1.25;
  }

  /* Near the 960px minimum the border note takes what's left of the meta line (its tooltip still
     has it all). */
  @media (max-width: 1100px) {
    .note + .meta {
      display: none;
    }
  }
</style>
