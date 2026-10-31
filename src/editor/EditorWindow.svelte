<script lang="ts">
  import { untrack } from 'svelte';
  import { api, type EditorMedia } from './api';
  import Lanes from './components/Lanes.svelte';
  import StatusBar from './components/StatusBar.svelte';
  import Toolbar from './components/Toolbar.svelte';
  import TopBar from './components/TopBar.svelte';
  import TrackHeaders from './components/TrackHeaders.svelte';
  import Viewer from './components/Viewer.svelte';
  import { PlaybackEngine } from './engine';
  import { fitZoom } from './geometry';
  import { keyToCommand, type Command } from './keys';
  import { STOPPED } from './playbackModel';
  import type { EditorStore } from './store.svelte';

  interface Props {
    store: EditorStore;
    onMinimize: () => void;
    onToggleMaximize: () => void;
    onClose: () => void;
    /** Mock only (`?mock=proxy`): act as if the video can't be decoded, to show the proxy step. */
    forceProxy?: boolean;
  }

  let { store, onMinimize, onToggleMaximize, onClose, forceProxy = false }: Props = $props();

  const FIT_MARGIN_PX = 24; // room after the last frame, so the end edge stays grabbable
  const PREVIEW_FAILED = "Couldn't show a preview of this video. Cuts and exports still work.";

  // A newly loaded clip fits the lanes as soon as their width is known.
  $effect(() => {
    if (!store.needsFit || !store.media || store.viewWidth <= 0) return;
    store.pxPerSecond = fitZoom(store.media.duration_s, store.viewWidth - FIT_MARGIN_PX);
    store.scrollX = 0;
    store.needsFit = false;
  });

  // --- playback ------------------------------------------------------------------------------

  let video = $state<HTMLVideoElement>();
  let engine = $state.raw<PlaybackEngine | null>(null);
  /** The last time the engine reported; any other playhead change is a seek. */
  let engineT = -1;
  /** The open clip's audio tracks, as handed to the engine. */
  let tracks: { index: number; src: string }[] = [];
  /** Whether the open clip has already gone to its H.264 proxy (so another failure is final). */
  let proxied = false;

  $effect(() => {
    if (!video) return;
    let ctx: AudioContext | null = null;
    try {
      ctx = new AudioContext();
    } catch (err) {
      console.warn('No Web Audio; tracks play without the gain stage', err);
    }
    const e = new PlaybackEngine(video, ctx);
    const offTime = e.onTime((t) => {
      engineT = t;
      store.playhead = t;
      store.follow();
    });
    const offEnded = e.onEnded(() => (store.shuttle = STOPPED));
    engine = e;
    return () => {
      offTime();
      offEnded();
      e.destroy();
      void ctx?.close();
      engine = null;
    };
  });

  // Order matters: the timeline first, then a new clip, then seeks, then the shuttle (so Space at
  // the end seeks back to 0 before it plays).
  $effect(() => {
    engine?.setTimeline(store.timeline);
  });

  $effect(() => {
    const e = engine;
    const m = store.media;
    if (e && m) untrack(() => openPreview(e, m));
  });

  $effect(() => {
    const t = store.playhead;
    const e = engine;
    if (e && t !== engineT) untrack(() => e.seek(t));
  });

  $effect(() => {
    const s = store.shuttle;
    const e = engine;
    if (!e) return;
    untrack(() => {
      e.setShuttle(s);
      // A stopped playhead sits on a frame, so a cut lands on the frame the viewer shows.
      if (s.dir === 0) store.seek(store.playhead);
    });
  });

  function isHevc(m: EditorMedia): boolean {
    return /^(hevc|h265)$/i.test(m.video?.codec ?? '');
  }

  /** Loads the clip's picture and tracks, or starts a proxy when this webview can't decode it. */
  function openPreview(e: PlaybackEngine, m: EditorMedia) {
    tracks = m.tracks
      .flatMap((t) => (!t.empty && t.file ? [{ index: t.index, src: api.src(t.file) }] : []))
      .filter((t) => t.src !== '');
    proxied = false;
    const src = m.video ? api.src(m.path) : '';
    const undecodable = isHevc(m) && video?.canPlayType('video/mp4; codecs="hvc1"') === '';
    if (forceProxy || (src && undecodable)) {
      e.load('', tracks);
      makeProxy(e, m);
    } else {
      e.load(src, tracks);
    }
  }

  function makeProxy(e: PlaybackEngine, m: EditorMedia) {
    if (proxied) return;
    proxied = true;
    store.proxyFraction = 0;
    const current = () => store.media?.session === m.session;
    api
      .proxy(m.session)
      .then((path) => {
        if (!current()) return;
        store.proxyFraction = null;
        e.load(api.src(path), tracks);
      })
      .catch((err) => {
        if (!current()) return;
        store.proxyFraction = null;
        store.error = typeof err === 'string' && err ? err : PREVIEW_FAILED;
      });
  }

  /** The picture failed to decode: go to the proxy once; if the proxy fails too, say so. */
  function onVideoError() {
    const m = store.media;
    if (!m?.video || !engine || !video?.getAttribute('src')) return;
    if (proxied) {
      if (store.proxyFraction === null) store.error = PREVIEW_FAILED;
      return;
    }
    console.warn('The video could not be decoded here; preparing a preview', video.error);
    makeProxy(engine, m);
  }

  // --- keyboard ------------------------------------------------------------------------------

  // Taps that mean "once": holding the key down shouldn't repeat them.
  const NO_REPEAT = new Set<Command>(['playPause', 'J', 'K', 'L', 'cut', 'delete', 'toggleSnap', 'toggleLinked']);
  // Keys a focused slider uses itself.
  const SLIDER_KEYS = new Set<Command>(['frameBack', 'frameFwd', 'secBack', 'secFwd', 'prevCut', 'nextCut', 'home', 'end']);
  const TEXT_TYPES = new Set(['text', 'search', 'email', 'url', 'tel', 'password', 'number']);

  function onKeydown(e: KeyboardEvent) {
    if (!store.media) return;
    const target = e.target instanceof HTMLElement ? e.target : null;
    if (target?.isContentEditable || target instanceof HTMLTextAreaElement) return;
    if (target instanceof HTMLInputElement && TEXT_TYPES.has(target.type)) return;
    const c = keyToCommand(e);
    if (!c) return;
    if (target instanceof HTMLInputElement && target.type === 'range' && SLIDER_KEYS.has(c)) return;
    // Also stops Space from pressing a focused button as well.
    e.preventDefault();
    if (e.repeat && NO_REPEAT.has(c)) return;
    store.command(c);
  }
</script>

<svelte:window onkeydown={onKeydown} />

<main class="win">
  <TopBar {store} {onMinimize} {onToggleMaximize} {onClose} />
  <Viewer {store} bind:video {onVideoError} />
  <Toolbar {store} />
  <div class="timeline">
    <TrackHeaders {store} />
    <Lanes {store} bind:width={store.viewWidth} />
  </div>
  <StatusBar {store} />
</main>

<style>
  .win {
    box-sizing: border-box;
    width: 100vw;
    height: 100vh;
    display: flex;
    flex-direction: column;
    background: var(--void);
    border: 1px solid var(--smoke);
    overflow: hidden;
    user-select: none;
    font-size: 13px;
    letter-spacing: -0.017em;
    /* Proportional figures in names; numeric readouts opt into tabular-nums themselves. */
    font-variant-numeric: normal;
    -webkit-font-smoothing: antialiased;
  }
  .win :global(*),
  .win :global(*::before),
  .win :global(*::after) {
    box-sizing: border-box;
  }
  /* Zero-specificity resets, so classes like .cap still set size and tracking. */
  :global(:where(.win) :where(button, input)) {
    font: inherit;
    letter-spacing: inherit;
    color: inherit;
  }
  :global(:where(.win) :where(button)) {
    cursor: pointer;
  }
  :global(:where(.win) :where(button:disabled)) {
    cursor: default;
  }

  .timeline {
    flex: none;
    max-height: 48%;
    min-height: 0;
    display: grid;
    grid-template-columns: 200px minmax(0, 1fr);
    align-items: start;
    overflow-x: hidden;
    overflow-y: auto;
  }
</style>
