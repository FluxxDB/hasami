import { mount } from 'svelte';
import { getCurrentWindow } from '@tauri-apps/api/window';
import '../lib/theme.css';
import { api, isMock } from './api';
import EditorWindow from './EditorWindow.svelte';
import { EditorStore } from './store.svelte';

// The Rust side opens this window as `editor.html` once a Quick Trim batch starts; the files to
// trim come from `editor_files`, and later batches arrive as `Files` events.
const store = new EditorStore();
const win = isMock ? null : getCurrentWindow();
const mockParam = new URLSearchParams(location.search).get('mock');

mount(EditorWindow, {
  target: document.getElementById('app')!,
  props: {
    store,
    onMinimize: () => (win ? void win.minimize() : console.info('[mock] window.minimize()')),
    onToggleMaximize: () => (win ? void win.toggleMaximize() : console.info('[mock] window.toggleMaximize()')),
    onClose: () => (win ? void win.close() : console.info('[mock] window.close()')),
    forceProxy: isMock && mockParam === 'proxy',
  },
});

function addFiles(paths: string[]) {
  for (const p of paths) if (!store.queue.files.includes(p)) store.queue.files.push(p);
}

async function openFile(path: string) {
  let media;
  try {
    media = await api.open(path);
  } catch (err) {
    store.error = typeof err === 'string' && err ? err : "Couldn't open this video.";
    return;
  }
  store.load(media);
  if (isMock && mockParam !== 'fresh') (await import('./mock')).demoEdits(store);

  // Thumbnails and waveforms fill in as they arrive (spec 6); a late answer for a clip that is
  // no longer open is dropped.
  const session = media.session;
  const current = () => store.media?.session === session;
  api
    .thumbs(session)
    .then((thumbs) => current() && (store.thumbs = thumbs))
    .catch((err) => console.error('editor_thumbs failed', err));
  for (const t of media.tracks.filter((t) => !t.empty)) {
    api
      .peaks(session, t.index)
      .then((p) => current() && (store.peaks = { ...store.peaks, [t.index]: p }))
      .catch((err) => console.error('editor_peaks failed', err));
  }
}

async function boot() {
  await api.onEvent((e) => {
    if (e.type === 'Files') addFiles(e.paths);
    else if (e.type === 'ProxyProgress' && e.session === store.media?.session && store.proxyFraction !== null) {
      store.proxyFraction = e.fraction;
    }
  });
  addFiles(await api.files());
  // Rust holds editor events until the listener is attached.
  api.ready().catch((err) => console.error('editor_ready failed', err));
  const first = store.queue.files[store.queue.index];
  if (first) await openFile(first);
}

boot();
