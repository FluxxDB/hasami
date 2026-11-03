<script lang="ts">
  import { formatPercent } from '../../progress/format';
  import { fileName, metaLine } from '../display';
  import type { Current, Direction, EditorStore, Result } from '../store.svelte';
  import { RESULTS_ID } from './Summary.svelte';

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
  const audio = $derived(store.audioExport);
  const audioBusy = $derived(audio?.state === 'queued' || audio?.state === 'running');
  const audioOk = $derived(idle && store.canExportAudio && !audioBusy);
  const NO_AUDIO = 'This clip has no audio to export';
  /** Said while the next clip opens, or when it couldn't be opened; empty once it's open. */
  const opening = $derived(media || !path ? '' : store.error ? "Couldn't open" : 'Opening…');

  // A Quick / Save raw export takes the export buttons' place while it runs, and once it's done.
  const video = $derived(store.videoExport);
  const showVideo = $derived(!!media && store.locked && !!video);
  const VIDEO_LABEL: Record<string, string> = { quick: 'Quick export', save_raw: 'Export + save raw' };

  // The row under the bar: an outcome that needs words (why an export failed, and that the
  // original is untouched; a saved export's note, e.g. that its 20 MB copy couldn't be made; or
  // Export audio's saved file). It stays until dismissed, the next export of the clip, or the
  // next clip.
  const KEPT = 'The original was not changed.';
  const outcome = $derived.by((): Result | null => {
    if (!media) return null;
    if (video?.state === 'failed' || (video?.state === 'done' && video.note)) return video;
    if (audio?.state === 'failed' || audio?.state === 'done') return audio;
    return null;
  });
  const outcomeKey = $derived(
    outcome ? `${store.visit}:${outcome.action}:${outcome.state}:${outcome.output ?? outcome.message}:${outcome.note ?? ''}` : '',
  );
  let dismissed = $state('');
  const showOutcome = $derived(!!outcome && outcomeKey !== dismissed);
  /** Read out once per change: queued, saved or failed (not every percent). */
  const announce = $derived.by(() => {
    const r = showVideo ? video : (outcome ?? (audioBusy ? audio : null));
    if (!r) return '';
    const what = r.action === 'audio' ? 'Export audio' : VIDEO_LABEL[r.action];
    if (r.state === 'done') return r.note ?? `Saved ${fileName(r.output ?? '')}`;
    if (r.state === 'failed') return `${r.message ?? ''} ${r.originalIntact ? KEPT : ''}`;
    return `${what} ${r.state === 'queued' ? 'queued' : 'running'}`;
  });

  const pct = (r: Result) => (r.state === 'queued' ? 'Queued' : formatPercent(r.fraction));

  // Prev / Next video: always here; where each goes is said in its tooltip. Next is newer, Prev older.
  const NOTHING: Record<Direction, string> = {
    next: 'No newer video in this folder',
    prev: 'No older video in this folder',
  };
  /** No tooltip while that way is still being checked (or the check failed: pressing it says why). */
  function stepTitle(dir: Direction, to: Current | null | undefined): string | undefined {
    if (to) return `${dir === 'next' ? 'Next' : 'Previous'}: ${fileName(to.path)}`;
    return to === undefined && path ? undefined : NOTHING[dir];
  }
  const nextOk = $derived(!store.busy && store.canNext);
  const prevOk = $derived(!store.busy && store.canPrev);
  const nextTitle = $derived(stepTitle('next', store.nextTarget));
  const prevTitle = $derived(stepTitle('prev', store.prevTarget));

  // The export strip opens the results list; it stays once there's anything to list.
  const results = $derived(store.results);
  const running = $derived(store.exports.running.length);
  const failedCount = $derived(results.filter((r) => r.state === 'failed').length);
  // Exports run one at a time, so the first one still running is the one making progress.
  const fraction = $derived(running > 0 ? (store.exports.progress[store.exports.running[0]] ?? 0) : 0);
  const strip = $derived(
    running > 0 ? `Exporting ${running}` : failedCount > 0 ? `Results · ${failedCount} failed` : 'Results',
  );
  const position = $derived(
    store.queueMode && store.current?.at != null ? `${store.current.at + 1} of ${store.queue.files.length}` : null,
  );
</script>

{#snippet arrow()}
  <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.2" aria-hidden="true">
    <path d="M3 9 9 3M4 3h5v5" />
  </svg>
{/snippet}

{#snippet forward()}
  <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.2" aria-hidden="true">
    <path d="M1.5 6h9M7 2.5 10.5 6 7 9.5" />
  </svg>
{/snippet}

{#snippet backward()}
  <svg width="11" height="11" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.2" aria-hidden="true">
    <path d="M10.5 6h-9M5 2.5 1.5 6 5 9.5" />
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
    aria-valuetext={pct(r)}
  >
    <span class="cap plabel">{label}</span>
    <span class="bar" aria-hidden="true"><i style:width="{Math.round(r.fraction * 100)}%"></i></span>
    <span class="cap pct">{pct(r)}</span>
  </div>
{/snippet}

<header class="topbar" data-tauri-drag-region>
  <div class="brand"><b>HASAMI</b><em>quick trim</em></div>
  <div class="vsep brand-sep"></div>
  <div class="file">
    {#if media}
      <!-- The name and meta line take the pointer for their tooltips (they're cut short in a
           narrow window) and are drag regions themselves, so the window still moves from them. -->
      <div class="name">
        <h1 class="fname" title={media.file_name} data-tauri-drag-region>{media.file_name}</h1>
        {#if store.edited}<span class="dot" aria-hidden="true"></span><span class="cap edited">Edited</span>{/if}
      </div>
      <div class="meta cap" title={metaLine(media)} data-tauri-drag-region>{metaLine(media)}</div>
    {:else if path}
      <!-- Opening the next clip, or it couldn't be opened (the viewer says why). -->
      <div class="name"><h1 class="fname" title={fileName(path)} data-tauri-drag-region>{fileName(path)}</h1></div>
      <div class="meta cap">{opening}</div>
    {/if}
    <!-- Always mounted, so a change of its text ("Opening…", "Couldn't open") is announced. -->
    <span class="sr-only" role="status">{opening}</span>
  </div>
  <div class="spacer" data-tauri-drag-region></div>

  <!-- The queue's place, Discard, and the export strip under them: stacked, so the file name
       keeps its room at the minimum window width. -->
  {#if path || results.length > 0}
    <div class="side">
      {#if path}
        <div class="queue">
          {#if position}<span class="pos cap">{position}</span>{/if}
          <!-- Not while the clip is still opening (the file is being read), nor once a video
               export is dealing with the original. -->
          <button
            type="button"
            class="link cap"
            disabled={store.busy || store.locked || (!media && !store.error)}
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
    <div class="vsep"></div>
  {/if}

  <span class="sr-only" role="status">{announce}</span>

  {#if showVideo && video}
    {#if video.state === 'done'}
      <div class="saved">
        <span class="cap plabel">Saved</span><span class="out" title={video.output}>{fileName(video.output ?? '')}</span>
      </div>
    {:else}
      {@render meter(video, VIDEO_LABEL[video.action])}
    {/if}
  {:else}
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
    {#if audio && audioBusy}
      <!-- Export audio's own progress, in its place; the clip stays editable meanwhile. -->
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

  <div class="vsep"></div>
  <!-- aria-disabled, so "No older video in this folder" still shows on hover and focus. -->
  <div class="steps">
    <button
      type="button"
      class="btn prev cap"
      aria-disabled={!prevOk}
      aria-label="Previous video"
      title={prevTitle}
      onclick={() => prevOk && store.prevVideo()}>{@render backward()} <span class="word">Prev</span></button
    >
    <button
      type="button"
      class="btn next cap"
      aria-disabled={!nextOk}
      aria-label="Next video"
      title={nextTitle}
      onclick={() => nextOk && store.nextVideo()}><span class="word">Next<span class="more">&nbsp;video</span></span> {@render forward()}</button
    >
  </div>

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

{#if showOutcome && outcome}
  <div class="outcome" class:failed={outcome.state === 'failed'}>
    {#if outcome.state === 'failed'}
      <span class="cap plabel">{outcome.action === 'audio' ? 'Export audio failed' : `${VIDEO_LABEL[outcome.action]} failed`}</span>
      <span class="msg">{outcome.message}</span>
      {#if outcome.originalIntact}<span class="kept">{KEPT}</span>{/if}
    {:else if outcome.note}
      <!-- The note names the saved file itself ("Saved …, but couldn't make the 20 MB copy: …"). -->
      <span class="cap plabel">{outcome.action === 'audio' ? 'Export audio' : VIDEO_LABEL[outcome.action]}</span>
      <span class="msg" title={outcome.note}>{outcome.note}</span>
    {:else}
      <span class="cap plabel">Saved</span><span class="out" title={outcome.output}>{fileName(outcome.output ?? '')}</span>
    {/if}
    <button type="button" class="ctl dismiss" aria-label="Dismiss" onclick={() => (dismissed = outcomeKey)}>
      <svg width="10" height="10" viewBox="0 0 12 12" aria-hidden="true"><path d="M2 2l8 8M10 2l-8 8" stroke="currentColor" fill="none" /></svg>
    </button>
  </div>
{/if}

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
  .out,
  .msg {
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

  /* The outcome row under the bar: one line of plain words, quiet but full width. */
  .outcome {
    flex: none;
    height: 32px;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 0 10px 0 18px;
    border-bottom: 1px solid var(--line);
    background: var(--void);
    white-space: nowrap;
  }
  .outcome .msg {
    flex: 0 1 auto;
  }
  .outcome .kept {
    flex: none;
    color: var(--pebble);
  }
  .outcome .dismiss {
    margin-left: auto;
    height: 24px;
  }

  .queue {
    display: flex;
    align-items: center;
    gap: 12px;
  }
  .steps {
    flex: none;
    display: flex;
    gap: 6px;
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

  /* Discard, the export strip and Next video share the bar with the export buttons: below a wide
     window, tighten it so the file name keeps some room. */
  @media (max-width: 1440px) {
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
  }

  /* Then the file name outranks the wordmark's serif half, and Next video's second word (its
     tooltip names where it goes). */
  @media (max-width: 1360px) {
    .brand em,
    .next .more {
      display: none;
    }
  }

  @media (max-width: 1180px) {
    .ctl {
      width: 30px;
    }
    .bar {
      width: 120px;
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
    .btn {
      padding: 10px 10px;
    }
    /* Prev and Next become a pair of arrows; their names stay in aria-label and the tooltip. */
    .steps .word {
      display: none;
    }
    .steps .btn {
      padding: 10px 11px;
    }
    .ctl {
      width: 28px;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    button,
    .rail i,
    .bar i,
    .switch,
    .switch::after {
      transition: none;
    }
  }
</style>
