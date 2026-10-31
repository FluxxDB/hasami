<script lang="ts">
  import { api } from '../api';
  import { formatTimecode, videoSourceAt } from '../playbackModel';
  import type { EditorStore } from '../store.svelte';
  import { videoEnd } from '../timeline';

  let { store }: { store: EditorStore } = $props();

  // Room around the frame: top padding, the gap above the transport, the transport, bottom padding.
  const PAD_TOP = 18;
  const GAP = 16;
  const TRANSPORT_H = 42;
  const PAD_BOTTOM = 10;
  const SIDE = 24;
  const MIN_TRANSPORT_W = 480;

  let w = $state(0);
  let h = $state(0);

  const media = $derived(store.media);
  const tl = $derived(store.timeline);
  const fps = $derived(tl.fps);
  const aspect = $derived(media?.video ? media.video.width / media.video.height : 16 / 9);

  // The frame is the largest box of the video's shape that fits; the transport matches its width.
  const frame = $derived.by(() => {
    const availW = Math.max(0, w - SIDE * 2);
    const availH = Math.max(0, h - PAD_TOP - GAP - TRANSPORT_H - PAD_BOTTOM);
    const fw = Math.min(availW, availH * aspect);
    return { w: fw, h: fw / aspect };
  });
  const transportW = $derived(Math.min(Math.max(frame.w, MIN_TRANSPORT_W), Math.max(0, w - 32)));

  const videoSrc = $derived(media ? api.src(media.path) : '');

  // Until the video has a frame to show, the nearest thumbnail stands in for it.
  const poster = $derived.by(() => {
    const thumbs = store.thumbs;
    if (thumbs.length === 0) return '';
    const srcT = videoSourceAt(tl, store.playhead)?.srcT ?? 0;
    let best = thumbs[0];
    for (const th of thumbs) if (Math.abs(th.t - srcT) < Math.abs(best.t - srcT)) best = th;
    return api.src(best.file);
  });

  const playing = $derived(store.shuttle.dir !== 0);
  const badge = $derived(
    store.shuttle.dir === -1 || store.shuttle.speed > 1
      ? `${store.shuttle.dir === -1 ? '−' : ''}${store.shuttle.speed}×`
      : null,
  );
</script>

<section class="viewer" aria-label="Viewer" bind:clientWidth={w} bind:clientHeight={h}>
  <div class="frame" style:width="{frame.w}px" style:height="{frame.h}px">
    {#if media?.video}
      <video src={videoSrc || undefined} poster={poster || undefined} muted playsinline preload="auto"></video>
    {/if}
    {#if store.error}
      <p class="message">{store.error}</p>
    {/if}
  </div>

  <div class="transport" style:width="{transportW}px">
    <div class="tcwrap">
      <span class="tc">{formatTimecode(store.playhead, fps)}</span>
      {#if badge}<span class="speed cap">{badge}</span>{/if}
    </div>
    <div class="tbtns">
      <button type="button" class="ibtn" aria-label="Jump to start" disabled={!media}>
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M19 20 9 12l10-8v16Z" /><path d="M5 19V5" /></svg>
      </button>
      <button type="button" class="ibtn" aria-label="Previous frame" disabled={!media}>
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m15 18-6-6 6-6" /></svg>
      </button>
      <button type="button" class="ibtn play" aria-label={playing ? 'Pause' : 'Play'} disabled={!media}>
        {#if playing}
          <svg width="15" height="15" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><rect x="6" y="4.5" width="4" height="15" rx="1" /><rect x="14" y="4.5" width="4" height="15" rx="1" /></svg>
        {:else}
          <svg width="15" height="15" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M7 4.5v15a.5.5 0 0 0 .77.42l11.5-7.5a.5.5 0 0 0 0-.84L7.77 4.08A.5.5 0 0 0 7 4.5Z" /></svg>
        {/if}
      </button>
      <button type="button" class="ibtn" aria-label="Next frame" disabled={!media}>
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m9 18 6-6-6-6" /></svg>
      </button>
      <button type="button" class="ibtn" aria-label="Jump to end" disabled={!media}>
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m5 4 10 8-10 8V4Z" /><path d="M19 5v14" /></svg>
      </button>
    </div>
    <span class="tc dim">{formatTimecode(videoEnd(tl), fps)}</span>
  </div>
</section>

<style>
  .viewer {
    flex: 1 1 auto;
    min-height: 160px;
    background: var(--black);
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 16px;
    padding: 18px 0 10px;
    overflow: hidden;
  }
  .frame {
    position: relative;
    flex: none;
    background: var(--black);
  }
  video {
    display: block;
    width: 100%;
    height: 100%;
    object-fit: contain;
    background: var(--black);
  }
  .message {
    position: absolute;
    inset: 0;
    margin: 0;
    display: grid;
    place-items: center;
    padding: 24px;
    text-align: center;
    color: var(--pebble);
    font-size: 14px;
  }

  .transport {
    flex: none;
    height: 42px;
    display: grid;
    grid-template-columns: 1fr auto 1fr;
    align-items: center;
  }
  .tcwrap {
    display: flex;
    align-items: center;
  }
  .tc {
    font-size: 14px;
    letter-spacing: 0;
    font-variant-numeric: tabular-nums;
  }
  .tc.dim {
    color: var(--pebble);
    text-align: right;
  }
  .speed {
    font-variant-numeric: tabular-nums;
    margin-left: 10px;
    padding: 3px 6px;
    border: 1px solid var(--iron);
    border-radius: 3px;
    line-height: 1.2;
  }

  .tbtns {
    display: flex;
    gap: 6px;
    align-items: center;
  }
  .ibtn {
    width: 32px;
    height: 32px;
    display: grid;
    place-items: center;
    padding: 0;
    border: 0;
    border-radius: 8px;
    color: var(--ash);
    background: transparent;
    transition:
      background-color 0.12s ease-out,
      color 0.12s ease-out;
  }
  .ibtn:hover:not(:disabled) {
    background: var(--charcoal);
    color: var(--bone);
  }
  /* The one raised control: the brand's graphite lift with its inset highlight. */
  .ibtn.play {
    width: 42px;
    height: 42px;
    background: var(--graphite);
    color: var(--bone);
    box-shadow: rgba(255, 255, 255, 0.2) 0 2px 5px 0 inset;
  }
  .ibtn.play:hover:not(:disabled) {
    background: var(--iron);
  }
  .ibtn:disabled {
    opacity: 0.4;
  }

  @media (prefers-reduced-motion: reduce) {
    .ibtn {
      transition: none;
    }
  }
</style>
