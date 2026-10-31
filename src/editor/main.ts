import { mount } from 'svelte';
import { getCurrentWindow } from '@tauri-apps/api/window';
import '../lib/theme.css';
import { api, isMock } from './api';
import EditorWindow from './EditorWindow.svelte';
import { EditorStore } from './store.svelte';

// The Rust side opens this window as `editor.html` once a Quick Trim batch starts; the files to
// trim come from `editor_files`, and later batches arrive as `Files` events. The store runs the
// queue (store.svelte.ts); this file wires it to the window.
const store = new EditorStore();
const win = isMock ? null : getCurrentWindow();
const mockParam = new URLSearchParams(location.search).get('mock');
// `?mock=1` and `?mock=proxy` show the approved mockup's edits; `fresh` and the queue mocks don't.
if (isMock && (mockParam === null || mockParam === '1' || mockParam === 'proxy')) {
  const demo = import('./mock');
  store.afterLoad = (s) => void demo.then((m) => m.demoEdits(s));
}

mount(EditorWindow, {
  target: document.getElementById('app')!,
  props: {
    store,
    onMinimize: () => (win ? void win.minimize() : console.info('[mock] window.minimize()')),
    onToggleMaximize: () => (win ? void win.toggleMaximize() : console.info('[mock] window.toggleMaximize()')),
    onClose: () => (win ? void win.close() : void mockClose()),
    forceProxy: isMock && mockParam === 'proxy',
  },
});

/** In a browser there's no close request to catch: ask the same question directly. */
async function mockClose() {
  if (store.edited && !(await store.ask('close'))) return;
  console.info('[mock] window.close()');
}

// A close request (the close button, Alt+F4, or the summary closing itself) with unsaved edits
// asks first, in the window. Exports already handed over keep running after it closes.
win?.onCloseRequested(async (event) => {
  if (!store.edited) return;
  event.preventDefault();
  if (await store.ask('close')) await win.destroy();
});

async function boot() {
  await api.onEvent((e) => store.applyEvent(e));
  store.addFiles(await api.files());
  // Rust holds editor events until the listener is attached.
  api.ready().catch((err) => console.error('editor_ready failed', err));
  const first = store.currentPath;
  if (first) await store.openFile(first);
}

boot().catch((err) => console.error('The editor could not start', err));
