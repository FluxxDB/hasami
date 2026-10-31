<script lang="ts">
  import { api, type Peaks, type Thumb } from '../api';
  import { LANE_BG, MUTED_COLOR, formatClock, mixHex, trackColor } from '../display';
  import { rulerStep, timeToX } from '../geometry';
  import type { EditorStore } from '../store.svelte';
  import { segLen, videoEnd, type Seg } from '../timeline';

  let { store, width = $bindable(0) }: { store: EditorStore; width?: number } = $props();

  const BONE = '#fcfcfc';
  const TILE_PX = 72; // thumbnail tile width in a video clip
  const BAR_PX = 3; // waveform bar pitch
  const EPS = 1e-6;

  const tl = $derived(store.timeline);
  const pps = $derived(store.pxPerSecond);
  const sx = $derived(store.scrollX);
  const vEnd = $derived(videoEnd(tl));
  const selected = $derived(new Set(store.selection));
  const empties = $derived(store.media?.tracks.filter((t) => t.empty) ?? []);
  const fileName = $derived(store.media?.file_name ?? '');

  const x = (t: number) => timeToX(t, pps, sx);

  /** A clip's box (1px inset each side, as in the mockup) and the part of it that's on screen. */
  function box(at: number, len: number) {
    const left = x(at) + 1;
    const w = Math.max(1, len * pps - 2);
    const visA = Math.max(0, -left);
    const visB = Math.min(w, width - left);
    return { left, w, visA, visB, onScreen: visB > visA };
  }

  const ticks = $derived.by(() => {
    if (width <= 0) return [];
    const { major, minor } = rulerStep(pps);
    const first = Math.max(0, Math.floor(sx / pps / minor));
    const last = Math.ceil((sx + width) / pps / minor);
    const out: { t: number; x: number; major: boolean; label: string }[] = [];
    for (let k = first; k <= last; k++) {
      const t = k * minor;
      const isMajor = Math.abs(t / major - Math.round(t / major)) < EPS;
      out.push({ t, x: x(t), major: isMajor, label: isMajor ? formatClock(t) : '' });
    }
    return out;
  });

  function nearestThumb(thumbs: Thumb[], t: number): Thumb | null {
    let best: Thumb | null = null;
    for (const th of thumbs) if (!best || Math.abs(th.t - t) < Math.abs(best.t - t)) best = th;
    return best;
  }

  /** The thumbnail tiles of a video clip that are on screen. */
  function tiles(seg: Seg, b: ReturnType<typeof box>) {
    const out: { left: number; url: string }[] = [];
    if (store.thumbs.length === 0) return out;
    for (let i = Math.floor(b.visA / TILE_PX); i * TILE_PX < b.visB; i++) {
      const th = nearestThumb(store.thumbs, seg.src[0] + (i * TILE_PX + TILE_PX / 2) / pps);
      if (th) out.push({ left: i * TILE_PX, url: api.src(th.file) });
    }
    return out;
  }

  /** Stretches of a track, from 0 to its last clip, with no audio: drawn as "Silence". */
  function silences(segs: Seg[]) {
    const out: { at: number; len: number }[] = [];
    let t = 0;
    for (const s of [...segs].sort((a, b) => a.at - b.at)) {
      if (s.at - t > EPS) out.push({ at: t, len: s.at - t });
      t = Math.max(t, s.at + segLen(s));
    }
    return out;
  }

  const overhangs = $derived(tl.audio.some((a) => a.segs.some((s) => s.at + segLen(s) > vEnd + EPS)));

  interface WaveParams {
    peaks: Peaks | undefined;
    srcStart: number; // source time at the canvas's left edge
    pps: number;
    width: number;
    color: string;
  }

  /** Draws a peak waveform as thin centred bars, one per BAR_PX, in the track color. */
  function waveform(canvas: HTMLCanvasElement, params: WaveParams) {
    const draw = (p: WaveParams) => {
      const dpr = window.devicePixelRatio || 1;
      const w = Math.max(1, Math.round(p.width));
      const h = canvas.clientHeight || 32;
      canvas.width = Math.round(w * dpr);
      canvas.height = Math.round(h * dpr);
      const ctx = canvas.getContext('2d');
      if (!ctx) return;
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, h);
      if (!p.peaks || p.peaks.values.length === 0) return;
      const { rate, values } = p.peaks;
      ctx.fillStyle = p.color;
      for (let bx = 0; bx < w; bx += BAR_PX) {
        const a = p.srcStart + bx / p.pps;
        const i0 = Math.max(0, Math.floor(a * rate));
        const i1 = Math.min(values.length, Math.max(i0 + 1, Math.ceil((a + BAR_PX / p.pps) * rate)));
        let v = 0;
        for (let i = i0; i < i1; i++) if (values[i] > v) v = values[i];
        const bh = Math.max(1, v * h);
        ctx.fillRect(bx, (h - bh) / 2, 1.6, bh);
      }
    };
    draw(params);
    return { update: draw };
  }
</script>

<div class="lanes" bind:clientWidth={width}>
  <div class="ruler" aria-hidden="true">
    {#each ticks as tk (tk.t)}
      <div class="tick" class:major={tk.major} style:left="{tk.x}px"></div>
      {#if tk.label}<span class="tick-label" style:left="{tk.x + 5}px">{tk.label}</span>{/if}
    {/each}
  </div>

  <div class="lane" aria-label="V1">
    {#each tl.video as seg (seg.id)}
      {@const b = box(seg.at, segLen(seg))}
      {#if b.onScreen}
        <div class="clip video" class:sel={selected.has(seg.id)} style:left="{b.left}px" style:width="{b.w}px">
          {#each tiles(seg, b) as tile (tile.left)}
            <div class="thumb" style:left="{tile.left}px" style:background-image={`url("${tile.url}")`}></div>
          {/each}
          <div class="label" style:left="{b.visA + 7}px">{fileName}</div>
        </div>
      {/if}
    {/each}
  </div>

  {#each tl.audio as tr, n (tr.index)}
    {@const c = tr.muted ? MUTED_COLOR : trackColor(n)}
    <div class="lane" aria-label="A{n + 1}" style:--c={c}>
      {#each silences(tr.segs) as gap (gap.at)}
        {@const b = box(gap.at, gap.len)}
        {#if b.onScreen}
          <div class="silence" style:left="{b.left}px" style:width="{b.w}px">
            {#if b.w > 72}<span class="cap">Silence</span>{/if}
          </div>
        {/if}
      {/each}
      {#each tr.segs as seg (seg.id)}
        {@const b = box(seg.at, segLen(seg))}
        {@const sel = selected.has(seg.id)}
        {#if b.onScreen}
          <div class="clip audio" class:sel style:left="{b.left}px" style:width="{b.w}px">
            <canvas
              style:left="{b.visA}px"
              style:width="{b.visB - b.visA}px"
              use:waveform={{
                peaks: store.peaks[tr.index],
                srcStart: seg.src[0] + (b.visA + 1) / pps,
                pps,
                width: b.visB - b.visA,
                color: sel ? mixHex(c, BONE, 0.45) : mixHex(c, LANE_BG, 0.88),
              }}
            ></canvas>
            {#if seg.at + segLen(seg) > vEnd + EPS}
              {@const from = Math.max(0, (vEnd - seg.at) * pps - 1)}
              <div class="overhang" style:left="{from}px"></div>
            {/if}
            <div class="label cap" style:left="{b.visA + 7}px">A{n + 1}</div>
          </div>
        {/if}
      {/each}
    </div>
  {/each}

  {#if empties.length > 0}
    <div class="empties"></div>
    {#if store.showEmpty}
      {#each empties as t (t.index)}<div class="lane empty"></div>{/each}
    {/if}
  {/if}

  {#if overhangs}
    <div class="video-end" style:left="{x(vEnd)}px"></div>
    <span class="video-end-label cap" style:left="{x(vEnd) + 8}px">Video ends</span>
  {/if}
  <div class="playhead" style:left="{x(store.playhead)}px"></div>
</div>

<style>
  .lanes {
    --lane-bg: #0b0b0b; /* LANE_BG in display.ts */
    position: relative;
    /* clip, not hidden: hidden would make this a scroll container and unstick the ruler. */
    overflow: clip;
    min-width: 0;
    background: var(--lane-bg);
  }

  .ruler {
    position: sticky;
    top: 0;
    z-index: 2;
    height: 28px;
    border-bottom: 1px solid var(--line);
    background: var(--void);
    overflow: hidden;
  }
  .tick {
    position: absolute;
    bottom: 0;
    width: 1px;
    height: 5px;
    background: #2c2b2c;
  }
  .tick.major {
    height: 11px;
    background: var(--iron);
  }
  .tick-label {
    position: absolute;
    bottom: 8px;
    font-size: 11px;
    letter-spacing: 0.02em;
    color: var(--pebble);
    white-space: nowrap;
    font-variant-numeric: tabular-nums;
  }

  .lane {
    position: relative;
    height: 64px;
    border-bottom: 1px solid var(--line);
  }
  .lane.empty {
    height: 40px;
  }
  .empties {
    height: 30px;
    border-bottom: 1px solid var(--line);
  }

  .clip {
    position: absolute;
    top: 6px;
    bottom: 6px;
    border-radius: 3px;
    overflow: hidden;
  }
  .clip.video {
    background: #111;
  }
  .thumb {
    position: absolute;
    top: 0;
    bottom: 0;
    width: 72px;
    background-size: cover;
    background-position: center;
    border-right: 1px solid rgba(0, 0, 0, 0.7);
    filter: saturate(0.85);
  }
  .clip .label {
    position: absolute;
    bottom: 4px;
    font-size: 11px;
    line-height: 1.2;
    letter-spacing: 0.02em;
    color: var(--bone);
    text-shadow: 0 1px 2px rgba(0, 0, 0, 0.95);
    white-space: nowrap;
    pointer-events: none;
  }
  /* The border (and the selection outline) sits above the thumbnails. */
  .clip.video::after {
    content: '';
    position: absolute;
    inset: 0;
    border-radius: 3px;
    box-shadow: inset 0 0 0 1px var(--graphite);
    pointer-events: none;
  }
  .clip.sel::after,
  .clip.audio.sel {
    box-shadow: inset 0 0 0 1.5px var(--bone);
  }

  .clip.audio {
    background: color-mix(in srgb, var(--c) 15%, var(--lane-bg));
    box-shadow: inset 0 0 0 1px color-mix(in srgb, var(--c) 38%, transparent);
  }
  .clip.audio.sel {
    background: color-mix(in srgb, var(--c) 28%, var(--lane-bg));
  }
  .clip.audio canvas {
    position: absolute;
    top: 5px;
    height: calc(100% - 20px);
  }
  .clip.audio .label {
    letter-spacing: 0.071em;
    color: color-mix(in srgb, var(--c) 55%, var(--bone));
    text-shadow: none;
  }
  .clip.audio.sel .label {
    color: var(--bone);
  }

  .silence {
    position: absolute;
    top: 6px;
    bottom: 6px;
    border-radius: 3px;
    border: 1px dashed var(--graphite);
    display: grid;
    place-items: center;
    overflow: hidden;
  }
  .silence span {
    color: var(--pebble);
    white-space: nowrap;
  }

  /* Audio past the end of the video: dropped on export, so hatched and dimmed. */
  .overhang {
    position: absolute;
    top: 0;
    bottom: 0;
    right: 0;
    background: repeating-linear-gradient(135deg, rgba(8, 8, 8, 0.78) 0 5px, rgba(8, 8, 8, 0.5) 5px 10px);
  }
  .video-end {
    position: absolute;
    top: 28px;
    bottom: 0;
    width: 1px;
    background: var(--ash);
    opacity: 0.55;
    pointer-events: none;
  }
  .video-end-label {
    position: absolute;
    top: 34px;
    color: var(--ash);
    white-space: nowrap;
    pointer-events: none;
  }

  .playhead {
    position: absolute;
    top: 0;
    bottom: 0;
    z-index: 3;
    width: 1px;
    background: var(--red);
    pointer-events: none;
  }
  .playhead::before {
    content: '';
    position: absolute;
    top: 0;
    left: -6px;
    width: 13px;
    height: 14px;
    background: var(--red);
    clip-path: polygon(0 0, 100% 0, 100% 60%, 50% 100%, 0 60%);
  }
</style>
