<script lang="ts">
  import { metaLine } from '../display';
  import type { EditorStore } from '../store.svelte';

  interface Props {
    store: EditorStore;
    onMinimize: () => void;
    onToggleMaximize: () => void;
    onClose: () => void;
  }

  let { store, onMinimize, onToggleMaximize, onClose }: Props = $props();

  const media = $derived(store.media);
</script>

{#snippet arrow()}
  <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.2" aria-hidden="true">
    <path d="M3 9 9 3M4 3h5v5" />
  </svg>
{/snippet}

<header class="topbar" data-tauri-drag-region>
  <div class="brand"><b>HASAMI</b><em>quick trim</em></div>
  <div class="vsep"></div>
  <div class="file">
    {#if media}
      <div class="name">
        <span class="fname">{media.file_name}</span>
        {#if store.edited}<span class="dot" aria-hidden="true"></span><span class="cap edited">Edited</span>{/if}
      </div>
      <div class="meta cap">{metaLine(media)}</div>
    {/if}
  </div>
  <div class="spacer" data-tauri-drag-region></div>

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
  <button type="button" class="link cap" disabled={!media}>Export audio</button>
  <button type="button" class="btn cap" disabled={!media}>Export + save raw {@render arrow()}</button>
  <button type="button" class="btn primary cap" disabled={!media}>Quick export {@render arrow()}</button>

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
  button:disabled {
    opacity: 0.4;
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
  .link:hover:not(:disabled) {
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
    .winctl {
      margin-left: 0;
    }
    .ctl {
      width: 30px;
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
    .switch,
    .switch::after {
      transition: none;
    }
  }
</style>
