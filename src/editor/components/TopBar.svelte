<script lang="ts">
  import { fileName, metaLine } from '../display';
  import type { EditorStore } from '../store.svelte';

  interface Props {
    store: EditorStore;
    onMinimize: () => void;
    onToggleMaximize: () => void;
    onClose: () => void;
  }

  let { store, onMinimize, onToggleMaximize, onClose }: Props = $props();

  const media = $derived(store.media);
  const path = $derived(store.currentPath);
  const idle = $derived(!!media && !store.busy);
  const audioOk = $derived(idle && store.canExportAudio);
  const NO_AUDIO = 'This clip has no audio to export';
  const running = $derived(store.exports.running.length);
  // Exports run one at a time, so the first one still running is the one making progress.
  const fraction = $derived(running > 0 ? (store.exports.progress[store.exports.running[0]] ?? 0) : 0);
</script>

{#snippet arrow()}
  <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.2" aria-hidden="true">
    <path d="M3 9 9 3M4 3h5v5" />
  </svg>
{/snippet}

<header class="topbar" class:queued={store.queueMode} data-tauri-drag-region>
  <div class="brand"><b>HASAMI</b><em>quick trim</em></div>
  <div class="vsep brand-sep"></div>
  <div class="file">
    {#if media}
      <div class="name">
        <span class="fname" title={media.file_name}>{media.file_name}</span>
        {#if store.edited}<span class="dot" aria-hidden="true"></span><span class="cap edited">Edited</span>{/if}
      </div>
      <div class="meta cap">{metaLine(media)}</div>
    {:else if path}
      <!-- Opening the next clip, or it couldn't be opened (the viewer says why). -->
      <div class="name"><span class="fname" title={fileName(path)}>{fileName(path)}</span></div>
      <div class="meta cap">{store.error ? "Couldn't open" : 'Opening…'}</div>
    {/if}
  </div>
  <div class="spacer" data-tauri-drag-region></div>

  <!-- The queue's place and controls, with the running exports under them: stacked, so the
       file name keeps its room at the minimum window width. -->
  {#if (store.queueMode && path) || running > 0}
    <div class="side">
      {#if store.queueMode && path}
        <div class="queue">
          <span class="pos cap">{store.queue.index + 1} of {store.queue.files.length}</span>
          <button type="button" class="link cap" disabled={store.busy} onclick={() => store.skip()}>Skip</button>
          <button type="button" class="link cap" disabled={store.busy} onclick={() => store.discard()}>Discard</button>
        </div>
      {/if}
      {#if running > 0}
        <div class="exporting cap" role="status">
          <span>Exporting {running}</span>
          <span class="rail" aria-hidden="true"><i style:width="{Math.round(fraction * 100)}%"></i></span>
        </div>
      {/if}
    </div>
    <div class="vsep"></div>
  {/if}

  <button
    type="button"
    class="toggle cap"
    role="switch"
    aria-checked={store.fit20}
    disabled={!media}
    onclick={() => (store.fit20 = !store.fit20)}
  >
    <span class="switch" class:on={store.fit20} aria-hidden="true"></span>Fit to 20 MB
  </button>
  <!-- aria-disabled rather than disabled, so the hint still shows on hover and focus. -->
  <button
    type="button"
    class="link cap"
    aria-disabled={!audioOk}
    title={media && !store.canExportAudio ? NO_AUDIO : undefined}
    aria-describedby={media && !store.canExportAudio ? 'no-audio-hint' : undefined}
    onclick={() => audioOk && store.exportClip('audio')}>Export audio</button
  >
  {#if media && !store.canExportAudio}<span id="no-audio-hint" class="sr-only">{NO_AUDIO}</span>{/if}
  <button type="button" class="btn cap" disabled={!idle} onclick={() => store.exportClip('save_raw')}
    >Export + save raw {@render arrow()}</button
  >
  <button type="button" class="btn primary cap" disabled={!idle} onclick={() => store.exportClip('quick')}
    >Quick export {@render arrow()}</button
  >

  <div class="winctl">
    <button type="button" class="ctl" aria-label="Minimize" onclick={onMinimize}>
      <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="M1 6h10" stroke="currentColor" fill="none" /></svg>
    </button>
    <button type="button" class="ctl" aria-label="Maximize" onclick={onToggleMaximize}>
      <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"
        ><rect x="1.5" y="1.5" width="9" height="9" fill="none" stroke="currentColor" /></svg
      >
    </button>
    <button type="button" class="ctl close" aria-label="Close" onclick={onClose}>
      <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"
        ><path d="M2 2l8 8M10 2l-8 8" stroke="currentColor" fill="none" /></svg
      >
    </button>
  </div>
</header>

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
  .brand,
  .file {
    pointer-events: none;
  }
  .brand {
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
  .file {
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
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
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
  .meta {
    color: var(--pebble);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    overflow: hidden;
    text-overflow: ellipsis;
    line-height: 1.25;
  }
  .spacer {
    flex: 1;
    align-self: stretch;
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
  button:disabled,
  button[aria-disabled='true'] {
    opacity: 0.4;
  }
  button[aria-disabled='true'] {
    cursor: default;
  }
  .sr-only {
    position: absolute;
    width: 1px;
    height: 1px;
    overflow: hidden;
    clip-path: inset(50%);
    white-space: nowrap;
  }

  .side {
    flex: none;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 2px;
  }
  .exporting {
    display: flex;
    align-items: center;
    gap: 10px;
    color: var(--ash);
    font-variant-numeric: tabular-nums;
    white-space: nowrap;
    pointer-events: none;
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

  .toggle {
    display: flex;
    align-items: center;
    gap: 9px;
    color: var(--ash);
    padding: 6px 2px;
  }
  .toggle:hover:not(:disabled) {
    color: var(--bone);
  }
  .switch {
    position: relative;
    width: 28px;
    height: 16px;
    border-radius: 100px;
    border: 1px solid var(--iron);
    transition: border-color 0.12s ease-out;
  }
  .switch::after {
    content: '';
    position: absolute;
    top: 3px;
    left: 3px;
    width: 8px;
    height: 8px;
    border-radius: 50%;
    background: var(--pebble);
    transition:
      transform 0.15s ease-out,
      background-color 0.15s ease-out;
  }
  .switch.on {
    border-color: var(--bone);
  }
  .switch.on::after {
    background: var(--bone);
    transform: translateX(12px);
  }

  .link {
    padding: 6px 4px;
    text-decoration: underline;
    text-decoration-color: var(--iron);
    text-underline-offset: 4px;
  }
  .link:hover:not(:disabled, [aria-disabled='true']) {
    text-decoration-color: var(--bone);
  }

  .btn {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    padding: 10px 16px;
    border-radius: 8px;
    border: 1px solid var(--ash);
  }
  .btn:hover:not(:disabled) {
    border-color: var(--bone);
    background: var(--charcoal);
  }
  .btn:active:not(:disabled) {
    background: var(--smoke);
  }
  .btn.primary {
    border-color: var(--red);
  }
  .btn.primary:hover:not(:disabled) {
    border-color: var(--red);
  }

  .winctl {
    flex: none;
    display: flex;
    gap: 2px;
    margin-left: 6px;
  }
  .ctl {
    width: 34px;
    height: 28px;
    display: grid;
    place-items: center;
    color: var(--pebble);
    border-radius: 3px;
  }
  .ctl:hover {
    background: var(--charcoal);
    color: var(--bone);
  }

  /* Near the 960px minimum window width, tighten the bar so the file name keeps some room. */
  @media (max-width: 1180px) {
    .topbar {
      gap: 10px;
      padding-left: 14px;
    }
    .btn {
      padding: 10px 12px;
      gap: 8px;
    }
    .link {
      padding: 6px 2px;
    }
    .queue {
      gap: 8px;
    }
    .winctl {
      margin-left: 0;
    }
    .ctl {
      width: 30px;
    }
  }

  /* The queue's controls take room from the file name; tighten sooner so the name stays readable. */
  @media (max-width: 1440px) {
    .topbar.queued {
      gap: 10px;
      padding-left: 14px;
    }
    .queued .btn {
      padding: 10px 12px;
      gap: 8px;
    }
    .queued .link {
      padding: 6px 2px;
    }
    .queued .winctl {
      margin-left: 0;
    }
  }

  /* Near the minimum width a queue needs the wordmark's room for the file name (the title bar
     and taskbar still say Hasami). */
  @media (max-width: 1100px) {
    .queued .brand,
    .queued .brand-sep {
      display: none;
    }
  }

  /* At the narrowest widths the file name outranks the wordmark's serif half. */
  @media (max-width: 1050px) {
    .brand em {
      display: none;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    button,
    .rail i,
    .switch,
    .switch::after {
      transition: none;
    }
  }
</style>
