<script lang="ts">
  import { MUTED_COLOR, formatDb, trackColor } from '../display';
  import { faderDb, faderPos } from '../geometry';
  import type { EditorStore } from '../store.svelte';
  import Slider from './Slider.svelte';

  let { store }: { store: EditorStore } = $props();

  const audio = $derived(store.timeline.audio);
  const empties = $derived(store.emptyTracks);
</script>

<div class="heads">
  <div class="ruler-head"></div>
  <div class="head">
    <div class="row"><span class="tag cap">V1</span><span class="tname cap">Video</span></div>
  </div>

  {#each audio as tr, n (tr.index)}
    {@const tag = `A${n + 1}`}
    <div class="head" class:muted={tr.muted} style:--c={tr.muted ? MUTED_COLOR : trackColor(n)}>
      <div class="row">
        <span class="tag cap">{tag}</span>
        <i class="chip" aria-hidden="true"></i>
        <span class="tname cap">Audio</span>
        <button
          type="button"
          class="mute"
          aria-label="Mute {tag}"
          aria-pressed={tr.muted}
          disabled={store.locked}
          onclick={() => store.toggleMute(tr.index)}
        >
          <svg
            width="13"
            height="13"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="1.6"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
          >
            <path
              d="M11 4.7a.7.7 0 0 0-1.2-.5L6.4 7.6A1.4 1.4 0 0 1 5.4 8H3a1 1 0 0 0-1 1v6a1 1 0 0 0 1 1h2.4a1.4 1.4 0 0 1 1 .4l3.4 3.4a.7.7 0 0 0 1.2-.5Z"
            />
            {#if tr.muted}<path d="m16 9 5 6M21 9l-5 6" />{:else}<path d="M16 9a5 5 0 0 1 0 6" />{/if}
          </svg>
        </button>
      </div>
      <div class="vol">
        <Slider
          value={faderPos(tr.gainDb)}
          label="{tag} volume"
          valuetext={formatDb(tr.gainDb)}
          disabled={store.locked}
          oninput={(p) => store.setGain(tr.index, faderDb(p), false)}
          onchange={(p) => store.setGain(tr.index, faderDb(p), true)}
        />
        <span class="db">{formatDb(tr.gainDb)}</span>
      </div>
    </div>
  {/each}

  {#if empties.length > 0}
    <div class="empties">
      <button
        type="button"
        class="more cap"
        aria-expanded={store.showEmpty}
        onclick={() => (store.showEmpty = !store.showEmpty)}
      >
        {#if store.showEmpty}
          Hide empty tracks
        {:else}
          +{empties.length} empty {empties.length === 1 ? 'track' : 'tracks'}
        {/if}
      </button>
    </div>
    {#if store.showEmpty}
      {#each empties as t (t.index)}
        <div class="head empty">
          <div class="row"><span class="tag cap"></span><span class="tname cap">Empty track</span></div>
        </div>
      {/each}
    {/if}
  {/if}
</div>

<style>
  .heads {
    border-right: 1px solid var(--line);
    background: var(--charcoal);
    min-height: 100%;
  }
  /* A control tabbed into view stops below the sticky ruler row instead of under it. */
  .heads :global(:is(button, input)) {
    scroll-margin-top: 36px;
  }
  .ruler-head {
    position: sticky;
    top: 0;
    z-index: 2;
    height: 28px;
    border-bottom: 1px solid var(--line);
    background: var(--charcoal);
  }
  .head {
    height: 64px;
    display: flex;
    flex-direction: column;
    justify-content: center;
    gap: 10px;
    padding: 0 12px;
    border-bottom: 1px solid var(--line);
  }
  .row {
    display: flex;
    align-items: center;
    gap: 10px;
  }
  .tag {
    width: 20px;
    flex: none;
    font-weight: 700;
    color: var(--pebble);
  }
  .tname {
    flex: 1;
    min-width: 0;
    color: var(--bone);
  }
  .muted .tname {
    color: var(--pebble);
  }
  .chip {
    width: 8px;
    height: 8px;
    border-radius: 2px;
    background: var(--c);
    flex: none;
  }
  .mute {
    width: 24px;
    height: 22px;
    flex: none;
    display: grid;
    place-items: center;
    padding: 0;
    border-radius: 3px;
    border: 1px solid var(--iron);
    color: var(--ash);
    background: transparent;
    transition:
      border-color 0.12s ease-out,
      color 0.12s ease-out;
  }
  .mute:hover:not(:disabled) {
    border-color: var(--ash);
    color: var(--bone);
  }
  /* A locked clip (its export is under way) takes no more edits. */
  .mute:disabled {
    opacity: 0.4;
  }
  .mute[aria-pressed='true'] {
    border-color: var(--ash);
    background: var(--smoke);
  }
  .vol {
    display: flex;
    align-items: center;
    gap: 10px;
    padding-left: 30px;
  }
  .db {
    width: 54px;
    flex: none;
    font-size: 11px;
    letter-spacing: 0.04em;
    color: var(--pebble);
    white-space: nowrap;
    text-align: right;
    font-variant-numeric: tabular-nums;
  }

  .empties {
    height: 30px;
    display: flex;
    align-items: center;
    padding: 0 8px;
    border-bottom: 1px solid var(--line);
  }
  .more {
    padding: 3px 4px;
    border: 0;
    border-radius: 3px;
    color: var(--pebble);
    background: transparent;
    transition: color 0.12s ease-out;
  }
  .more:hover {
    color: var(--bone);
  }
  .head.empty {
    height: 40px;
  }
  .head.empty .tname {
    color: var(--pebble);
  }

  @media (prefers-reduced-motion: reduce) {
    .mute,
    .more {
      transition: none;
    }
  }
</style>
