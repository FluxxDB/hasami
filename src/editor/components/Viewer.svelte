<script lang="ts">
  import { api } from '../api';
  import { cropPlacement, nearestThumb } from '../display';
  import { formatTimecode, videoSourceAt } from '../playbackModel';
  import type { EditorStore } from '../store.svelte';
  import { videoEnd } from '../timeline';

  interface Props {
    store: EditorStore;
    /** The picture element; the playback engine owns its source. */
    video?: HTMLVideoElement;
    onVideoError?: () => void;
  }

  let { store, video = $bindable(), onVideoError }: Props = $props();

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
  // While a black border is cropped off (spec 5.7), the frame takes the shape of the picture inside
  // it, and the whole picture is placed so only that part shows: the <video> itself is untouched,
  // so the engine's seeking and the proxy work as ever. (`object-view-box` would be simpler, but
  // Chromium doesn't apply it to the poster, which stands in until the first frame.)
  const crop = $derived(store.crop);
  const place = $derived(crop && media?.video ? cropPlacement(crop, media.video) : null);
  const aspect = $derived(
    crop ? crop.w / crop.h : media?.video ? media.video.width / media.video.height : 16 / 9,
  );

  // The frame is the largest box of the video's shape that fits; the transport matches its width.
  const frame = $derived.by(() => {
    const availW = Math.max(0, w - SIDE * 2);
    const availH = Math.max(0, h - PAD_TOP - GAP - TRANSPORT_H - PAD_BOTTOM);
    const fw = Math.min(availW, availH * aspect);
    return { w: fw, h: fw / aspect };
  });
  const transportW = $derived(Math.min(Math.max(frame.w, MIN_TRANSPORT_W), Math.max(0, w - 32)));

  // Until the video has a frame to show, the nearest thumbnail stands in for it.
  const poster = $derived.by(() => {
    const th = nearestThumb(store.thumbs, videoSourceAt(tl, store.playhead)?.srcT ?? 0);
    return th ? api.src(th.file) : '';
  });

  const playing = $derived(store.shuttle.dir !== 0);
  const preparing = $derived(store.proxyFraction);
  const badge = $derived(
    store.shuttle.dir === -1 || store.shuttle.speed > 1
      ? `${store.shuttle.dir === -1 ? '−' : ''}${store.shuttle.speed}×`
      : null,
  );
</script>

<section class="viewer" aria-label="Viewer" bind:clientWidth={w} bind:clientHeight={h}>
  <div class="frame" style:width="{frame.w}px" style:height="{frame.h}px">
    <!-- Always present (hidden for audio-only files), because the engine's clock lives on it. -->
    <video
      bind:this={video}
      class:none={!media?.video}
      style:left={place ? `${place.left}%` : undefined}
      style:top={place ? `${place.top}%` : undefined}
      style:width={place ? `${place.width}%` : undefined}
      style:height={place ? `${place.height}%` : undefined}
      poster={poster || undefined}
      muted
      playsinline
      preload="auto"
      onerror={() => onVideoError?.()}
    ></video>
    {#if preparing !== null}
      <div class="preparing" role="status">
        <span class="cap">Preparing preview</span>
        <div class="meter" aria-hidden="true"><i style:width="{Math.round(preparing * 100)}%"></i></div>
        <span class="pct">{Math.round(preparing * 100)}%</span>
      </div>
    {/if}
    {#if store.error}
      <p class="message" role="alert">{store.error}</p>
    {:else if store.ended}
      <!-- The last video was discarded: the window stays (spec 5.7), with the results list. -->
      <p class="message" role="status">No more videos to trim here.</p>
    {/if}
  </div>

  <div class="transport" style:width="{transportW}px">
    <div class="tcwrap">
      <span class="tc">{formatTimecode(store.playhead, fps)}</span>
      {#if badge}<span class="speed cap">{badge}</span>{/if}
    </div>
    <div class="tbtns">
      <button type="button" class="ibtn" aria-label="Jump to start" disabled={!media} onclick={() => store.command('home')}>
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M19 20 9 12l10-8v16Z" /><path d="M5 19V5" /></svg>
      </button>
      <button type="button" class="ibtn" aria-label="Previous frame" disabled={!media} onclick={() => store.command('frameBack')}>
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m15 18-6-6 6-6" /></svg>
      </button>
      <button type="button" class="ibtn play" aria-label={playing ? 'Pause' : 'Play'} disabled={!media} onclick={() => store.command('playPause')}>
        {#if playing}
          <svg width="15" height="15" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><rect x="6" y="4.5" width="4" height="15" rx="1" /><rect x="14" y="4.5" width="4" height="15" rx="1" /></svg>
        {:else}
          <svg width="15" height="15" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="M7 4.5v15a.5.5 0 0 0 .77.42l11.5-7.5a.5.5 0 0 0 0-.84L7.77 4.08A.5.5 0 0 0 7 4.5Z" /></svg>
        {/if}
      </button>
      <button type="button" class="ibtn" aria-label="Next frame" disabled={!media} onclick={() => store.command('frameFwd')}>
        <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="m9 18 6-6-6-6" /></svg>
      </button>
      <button type="button" class="ibtn" aria-label="Jump to end" disabled={!media} onclick={() => store.command('end')}>
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
    overflow: hidden; /* the cropped-off border */
  }
  video {
    display: block;
    position: absolute;
    left: 0;
    top: 0;
    width: 100%;
    height: 100%;
    object-fit: contain;
    background: var(--black);
  }
  video.none {
    display: none;
  }

  /* Shown while an H.264 stand-in is made for a video this webview can't decode. */
  .preparing {
    position: absolute;
    inset: 0;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 12px;
    background: rgba(0, 0, 0, 0.72);
    color: var(--bone);
  }
  .meter {
    width: 180px;
    height: 1px;
    background: var(--iron);
  }
  .meter i {
    display: block;
    height: 1px;
    background: var(--bone);
    transition: width 0.2s ease-out;
  }
  .pct {
    font-size: 11px;
    letter-spacing: 0.04em;
    color: var(--pebble);
    font-variant-numeric: tabular-nums;
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
    .ibtn,
    .meter i {
      transition: none;
    }
  }
</style>
