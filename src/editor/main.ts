import { mount } from 'svelte';
import { getCurrentWindow } from '@tauri-apps/api/window';
import '../lib/theme.css';
import { isMock } from './api';
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

/** Runs a window call; a failure (the window is already going away, say) is only logged. */
const windowCall = (name: string, call: () => Promise<unknown>) => () =>
  void call().catch((err) => console.error(`window.${name}() failed`, err));

mount(EditorWindow, {
  target: document.getElementById('app')!,
  props: {
    store,
    onMinimize: win ? windowCall('minimize', () => win.minimize()) : () => console.info('[mock] window.minimize()'),
    onToggleMaximize: win
      ? windowCall('toggleMaximize', () => win.toggleMaximize())
      : () => console.info('[mock] window.toggleMaximize()'),
    onClose: win ? windowCall('close', () => win.close()) : () => void mockClose(),
    forceProxy: isMock && mockParam === 'proxy',
  },
});

/** In a browser there's no close request to catch: ask the same question directly. */
async function mockClose() {
  if (store.unsaved && !(await store.ask('close'))) return;
  console.info('[mock] window.close()');
}

// A close request (the close button or Alt+F4) with edits no export was made from asks first, in
// the window. The window never closes by itself; exports already handed over keep running after
// it closes, so they're no reason to ask.
win
  ?.onCloseRequested(async (event) => {
    if (!store.unsaved) return;
    event.preventDefault();
    if (await store.ask('close')) await win.destroy();
  })
  .catch((err) => console.error('Listening for the close request failed', err));

void store.start();
