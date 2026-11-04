<script lang="ts">
  // The export settings (20 MB copy, and Keep border when the clip has a border) and the export
  // buttons: Export audio, Export + save raw, Quick export (spec 5.7). A Quick / Save raw export
  // takes their place while it runs, and once it's done; Export audio's own progress takes only
  // its link's place, and the clip stays editable meanwhile.

  import { fileName } from '../../display';
  import { inFlight, type Result } from '../../exportLog.svelte';
  import type { EditorStore } from '../../store.svelte';
  import { LABEL, percentText } from './topbar';

  let { store, borderTitle }: { store: EditorStore; borderTitle?: string } = $props();

  const NO_AUDIO = 'This clip has no audio to export';

  const media = $derived(store.media);
  const idle = $derived(!!media && !store.busy);
  const audio = $derived(store.audioExport);
  const audioBusy = $derived(inFlight(audio));
  const audioOk = $derived(idle && store.canExportAudio && !audioBusy);
  const video = $derived(store.videoExport);
  const showVideo = $derived(!!media && store.locked && !!video);
</script>

{#snippet arrow()}
  <svg
    width="11"
    height="11"
    viewBox="0 0 12 12"
    fill="none"
    stroke="currentColor"
    stroke-width="1.2"
    aria-hidden="true"
  >
    <path d="M3 9 9 3M4 3h5v5" />
  </svg>
{/snippet}

{#snippet meter(r: Result, label: string)}
  <div
    class="progress"
    role="progressbar"
    aria-label={label}
    aria-valuemin={0}
    aria-valuemax={100}
    aria-valuenow={r.state === 'queued' ? undefined : Math.round(r.fraction * 100)}
    aria-valuetext={percentText(r)}
  >
    <span class="cap plabel">{label}</span>
    <span class="bar" aria-hidden="true"><i style:width="{Math.round(r.fraction * 100)}%"></i></span>
    <span class="cap pct">{percentText(r)}</span>
  </div>
{/snippet}

{#if showVideo && video}
  {#if video.state === 'done'}
    <div class="saved">
      <span class="cap plabel">Saved</span><span class="out" title={video.output}>{fileName(video.output ?? '')}</span>
    </div>
  {:else}
    {@render meter(video, LABEL[video.action])}
  {/if}
{:else}
  <div class="opts" class:two={!!store.border}>
    <button
      type="button"
      class="toggle cap"
      role="switch"
      aria-checked={store.fit20}
      disabled={!media}
      title="Video exports also save a copy of 20 MB or less to your Videos folder"
      onclick={() => (store.fit20 = !store.fit20)}
    >
      <span class="switch" class:on={store.fit20} aria-hidden="true"></span>20 MB copy
    </button>
    {#if store.border}
      <button
        type="button"
        class="toggle cap"
        role="switch"
        aria-checked={store.keepBorder}
        title={borderTitle}
        onclick={() => (store.keepBorder = !store.keepBorder)}
      >
        <span class="switch" class:on={store.keepBorder} aria-hidden="true"></span>Keep border
      </button>
    {/if}
  </div>
  {#if audio && audioBusy}
    <div class="audio-meter">{@render meter(audio, 'Audio')}</div>
  {:else}
    <!-- aria-disabled rather than disabled, so the hint still shows on hover and focus. -->
    <button
      type="button"
      class="link cap"
      aria-disabled={!audioOk}
      title={media && !store.canExportAudio ? NO_AUDIO : undefined}
      aria-describedby={media && !store.canExportAudio ? 'no-audio-hint' : undefined}
      onclick={() => audioOk && store.exportClip('audio')}>Export audio</button
    >
  {/if}
  {#if media && !store.canExportAudio}<span id="no-audio-hint" class="sr-only">{NO_AUDIO}</span>{/if}
  <button type="button" class="btn cap" disabled={!idle} onclick={() => store.exportClip('save_raw')}
    >Export + save raw {@render arrow()}</button
  >
  <button type="button" class="btn primary cap" disabled={!idle} onclick={() => store.exportClip('quick')}
    >Quick export {@render arrow()}</button
  >
{/if}

<style>
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

  /* An export's progress: a thin bone fill on a neutral track, with its percentage. */
  .progress {
    flex: none;
    display: flex;
    align-items: center;
    gap: 12px;
    white-space: nowrap;
  }
  .plabel {
    color: var(--pebble);
    flex: none;
  }
  .bar {
    width: 160px;
    height: 2px;
    background: var(--iron);
  }
  .bar i {
    display: block;
    height: 100%;
    background: var(--bone);
    transition: width 0.2s ease-out;
  }
  .pct {
    min-width: 6ch;
    font-variant-numeric: tabular-nums;
    color: var(--bone);
  }
  .saved {
    display: flex;
    align-items: baseline;
    gap: 10px;
    min-width: 0;
    flex: 0 1 auto;
    max-width: 380px;
  }
  .out {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .audio-meter .progress {
    gap: 8px;
  }
  .audio-meter .bar {
    width: 56px;
  }

  /* The export settings: 20 MB copy, and Keep border under it when the clip has a border. */
  .opts {
    flex: none;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 2px;
  }
  .opts.two .toggle {
    padding: 2px;
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

  /* 8px outline buttons; Quick export's border is the red. */
  .btn {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    padding: 10px 16px;
    border-radius: 8px;
    border: 1px solid var(--ash);
  }
  .btn:hover:not(:disabled, [aria-disabled='true']) {
    border-color: var(--bone);
    background: var(--charcoal);
  }
  .btn:active:not(:disabled, [aria-disabled='true']) {
    background: var(--smoke);
  }
  .btn.primary {
    border-color: var(--red);
  }
  .btn.primary:hover:not(:disabled, [aria-disabled='true']) {
    border-color: var(--red);
  }

  @media (max-width: 1440px) {
    .btn {
      padding: 10px 12px;
      gap: 8px;
    }
    .link {
      padding: 6px 2px;
    }
  }
  @media (max-width: 1180px) {
    .bar {
      width: 120px;
    }
  }
  @media (max-width: 1100px) {
    .btn {
      padding: 10px 10px;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    button,
    .bar i,
    .switch,
    .switch::after {
      transition: none;
    }
  }
</style>
