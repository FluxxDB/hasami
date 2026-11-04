<script lang="ts">
  // The small progress window for right-click Compress / Convert jobs (and editor exports whose
  // window has closed): one job at a time, from its job events (state.ts), with Cancel, the
  // low-resolution question, and what went wrong. It closes itself 1.5 s after a clean finish.

  import { untrack } from 'svelte';
  import type { JobEvent } from '../lib/jobs';
  import { doneLine, formatBytes, formatFps, progressLine } from './format';
  import { initialState, reduce, type Kind } from './state';

  interface Props {
    kind: Kind;
    ext?: string | null;
    /** Subscribe to job events; resolves to an unsubscribe function. */
    listen: (cb: (event: JobEvent) => void) => Promise<() => void>;
    onCancel: (id: number) => void;
    onConfirm: (id: number, proceed: boolean) => void;
    onClose: () => void;
    onMinimize: () => void;
  }

  let { kind, ext = null, listen, onCancel, onConfirm, onClose, onMinimize }: Props = $props();

  let view = $state(
    initialState(
      untrack(() => kind),
      untrack(() => ext),
    ),
  );

  $effect(() => {
    let off: (() => void) | undefined;
    let disposed = false;
    listen((event) => {
      view = reduce(view, event);
    })
      .then((fn) => {
        if (disposed) fn();
        else off = fn;
      })
      .catch((err) => console.error('Listening for job events failed', err));
    return () => {
      disposed = true;
      off?.();
    };
  });

  // Close 1.5 s after the queue drained with nothing to read; any new job cancels the timer.
  $effect(() => {
    if (!view.shouldClose) return;
    const timer = setTimeout(onClose, 1500);
    return () => clearTimeout(timer);
  });

  const percent = $derived(Math.floor(Math.min(1, Math.max(0, view.fraction)) * 100));
  const planText = $derived(
    view.kind === 'compress'
      ? [
          '≤ 20 MB',
          ...(view.target ? [`${view.target.width}×${view.target.height}`, `${formatFps(view.target.fps)} fps`] : []),
        ].join(' · ')
      : view.ext
        ? `.${view.ext}`
        : '',
  );
  const attention = $derived(view.phase === 'failed' || view.phase === 'unchanged' || view.phase === 'noted');
  const shownName = $derived(
    view.phase === 'failed'
      ? (view.failure?.fileName ?? view.fileName)
      : view.phase === 'unchanged'
        ? (view.notice?.fileName ?? view.fileName)
        : view.phase === 'noted'
          ? (view.note?.fileName ?? view.fileName)
          : view.fileName,
  );
</script>

<main class="win">
  <header class="bar" data-tauri-drag-region>
    <div class="brand" data-tauri-drag-region>
      <b>HASAMI</b><em>{view.kind}</em>
    </div>
    <div class="spacer" data-tauri-drag-region></div>
    <div class="winctl">
      <button type="button" class="ctl" aria-label="Minimize" onclick={onMinimize}>
        <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"
          ><path d="M1 6h10" stroke="currentColor" fill="none" /></svg
        >
      </button>
      <button type="button" class="ctl" aria-label="Close" onclick={onClose}>
        <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"
          ><path d="M2 2l8 8M10 2l-8 8" stroke="currentColor" fill="none" /></svg
        >
      </button>
    </div>
  </header>

  <section class="body" aria-live="polite">
    <div class="namerow">
      <div class="fname" title={shownName}>{shownName || 'Starting…'}</div>
      {#if view.phase === 'failed' && view.failedCount > 1}
        <span class="cap file">{view.failedCount} files failed</span>
      {:else if view.fileLabel && !attention}
        <span class="cap file">{view.fileLabel}</span>
      {/if}
    </div>

    {#if view.phase === 'failed'}
      <p class="message">{view.failure?.message}</p>
      <div class="foot">
        <span class="sub"
          >{#if view.failure?.originalIntact !== false}The original file was not changed.{/if}</span
        >
        <button type="button" class="btn cap" onclick={onClose}>Close</button>
      </div>
    {:else if view.phase === 'unchanged'}
      <p class="message">{view.notice?.reason}</p>
      <div class="foot">
        <span class="sub">The original file was not changed.</span>
        <button type="button" class="btn cap" onclick={onClose}>Close</button>
      </div>
    {:else if view.phase === 'noted'}
      <!-- Saved: the message itself says so. No "original not changed" line, which a Quick
           export would make untrue. -->
      <p class="message">{view.note?.message}</p>
      <div class="foot">
        <span class="sub"></span>
        <button type="button" class="btn cap" onclick={onClose}>Close</button>
      </div>
    {:else if view.phase === 'confirm' && view.target && view.id !== null}
      <p class="message">
        This would end up at {view.target.width}×{view.target.height}. Continue?
      </p>
      <div class="foot end">
        <button type="button" class="btn cap" onclick={() => onConfirm(view.id!, true)}>Continue</button>
        <button type="button" class="btn cap" onclick={() => onConfirm(view.id!, false)}>Cancel</button>
      </div>
    {:else}
      <div class="sizes cap">
        {#if view.phase === 'done' && view.outputBytes !== null}
          <b>{formatBytes(view.inputBytes)}</b> → <b>{formatBytes(view.outputBytes)}</b>
        {:else if view.phase !== 'waiting'}
          <b>{formatBytes(view.inputBytes)}</b> →
          {planText}
        {/if}
      </div>
      <div
        class="prog"
        role="progressbar"
        aria-label="Progress"
        aria-valuemin="0"
        aria-valuemax="100"
        aria-valuenow={percent}
      >
        <i style:width="{percent}%"></i>
      </div>
      <div class="foot cap">
        <span class="status">
          {#if view.phase === 'done'}{doneLine(
              view.kind,
            )}{:else if view.phase === 'cancelled'}Cancelled{:else if view.phase === 'waiting'}Starting…{:else}{progressLine(
              { pass: view.pass, passes: view.passes, fraction: view.fraction, etaS: view.etaS },
            )}{/if}
        </span>
        {#if view.phase === 'running' && view.id !== null}
          <button type="button" class="btn cap" onclick={() => onCancel(view.id!)}>Cancel</button>
        {/if}
      </div>
    {/if}
  </section>
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
  }

  .bar {
    flex: none;
    height: 40px;
    box-sizing: border-box;
    display: flex;
    align-items: center;
    padding: 0 6px 0 16px;
    border-bottom: 1px solid var(--line);
  }
  .brand {
    display: flex;
    align-items: baseline;
    gap: 7px;
    white-space: nowrap;
    pointer-events: none;
  }
  .brand b {
    font-weight: 700;
    font-size: 13px;
    letter-spacing: -0.03em;
  }
  .brand em {
    font-family: var(--serif);
    font-size: 15px;
    color: var(--pebble);
    letter-spacing: -0.01em;
  }
  .spacer {
    flex: 1;
    align-self: stretch;
  }
  .winctl {
    display: flex;
    gap: 2px;
  }
  .ctl {
    width: 34px;
    height: 28px;
    display: grid;
    place-items: center;
    padding: 0;
    color: var(--pebble);
    background: transparent;
    border: 0;
    border-radius: 3px;
    cursor: default;
    transition:
      background-color 0.12s ease-out,
      color 0.12s ease-out;
  }
  .ctl:hover {
    background: var(--charcoal);
    color: var(--bone);
  }

  .body {
    flex: 1;
    min-height: 0;
    box-sizing: border-box;
    padding: 18px 20px 16px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .namerow {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 16px;
  }
  .fname {
    font-size: 14px;
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .file {
    flex: none;
    color: var(--pebble);
  }
  .sizes {
    color: var(--pebble);
    min-height: 16px;
  }
  .sizes b {
    color: var(--bone);
    font-weight: 500;
  }

  .prog {
    height: 2px;
    background: var(--smoke);
    position: relative;
    flex: none;
  }
  .prog i {
    position: absolute;
    left: 0;
    top: 0;
    bottom: 0;
    background: var(--bone);
    transition: width 0.25s linear;
  }

  .foot {
    margin-top: auto;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    color: var(--pebble);
    min-height: 34px;
  }
  .foot.end {
    justify-content: flex-end;
  }
  .status {
    white-space: nowrap;
  }

  .message {
    margin: 0;
    font-size: 14px;
    color: var(--bone);
    user-select: text;
    overflow-wrap: anywhere;
    flex: 1 1 auto;
    min-height: 0;
    overflow-y: auto;
    line-height: 1.5;
  }
  .sub {
    font-size: 13px;
    color: var(--pebble);
  }

  .btn {
    padding: 8px 14px;
    color: var(--bone);
    background: transparent;
    border: 1px solid var(--ash);
    border-radius: 8px;
    font-family: inherit;
    cursor: pointer;
    transition:
      border-color 0.12s ease-out,
      background-color 0.12s ease-out;
  }
  .btn:hover {
    border-color: var(--bone);
    background: var(--charcoal);
  }
  .btn:active {
    background: var(--smoke);
  }
  .foot.end .btn + .btn {
    margin-left: 8px;
  }

  @media (prefers-reduced-motion: reduce) {
    .prog i,
    .btn,
    .ctl {
      transition: none;
    }
  }
</style>
