// The Rust side opens this window as `progress.html?kind=compress|convert[&ext=mov]`.
// `kind` (and `ext`, for convert) only seed the window; a `Started` event that carries
// its own `kind`/`ext` overrides them, which keeps a reused window right across mixed queues.

import { mount } from 'svelte';
import { invoke } from '@tauri-apps/api/core';
import { getCurrentWindow } from '@tauri-apps/api/window';
import '../lib/theme.css';
import { cancelJob, confirmLowRes, onJobEvent } from '../lib/jobs';
import ProgressWindow from './ProgressWindow.svelte';
import type { Kind } from './state';

const params = new URLSearchParams(location.search);
const kind: Kind = params.get('kind') === 'convert' ? 'convert' : 'compress';
const ext = params.get('ext')?.replace(/^\./, '') || null;

const hasTauri = '__TAURI_INTERNALS__' in window;
const mock = import.meta.env.DEV && !hasTauri ? (params.get('mock') ?? 'running') : null;

const target = document.getElementById('app')!;

async function start() {
  if (mock) {
    const { mockListen } = await import('./mock');
    return mount(ProgressWindow, {
      target,
      props: {
        kind: mock === 'convert' ? 'convert' : kind,
        ext,
        listen: mockListen(mock),
        onCancel: (id) => console.info('[mock] cancel_job', { id }),
        onConfirm: (id, proceed) => console.info('[mock] confirm_low_res', { id, proceed }),
        onClose: () => console.info('[mock] window.close()'),
        onMinimize: () => console.info('[mock] window.minimize()'),
      },
    });
  }
  const win = getCurrentWindow();
  return mount(ProgressWindow, {
    target,
    props: {
      kind,
      ext,
      // Rust buffers job events until this window is listening; tell it once the listener is attached.
      listen: (cb) =>
        onJobEvent(cb).then((off) => {
          invoke('progress_ready').catch((err) => console.error('progress_ready failed', err));
          return off;
        }),
      onCancel: (id) => void cancelJob(id).catch((err) => console.error('cancel_job failed', err)),
      onConfirm: (id, proceed) =>
        void confirmLowRes(id, proceed).catch((err) => console.error('confirm_low_res failed', err)),
      onClose: () => void win.close().catch((err) => console.error('window.close() failed', err)),
      onMinimize: () => void win.minimize().catch((err) => console.error('window.minimize() failed', err)),
    },
  });
}

start().catch((err) => console.error('The progress window could not start', err));
