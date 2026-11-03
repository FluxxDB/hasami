<script lang="ts">
  import type { ConfirmKind, EditorStore } from '../store.svelte';

  // The unsaved-edits question (close, Prev or Next video with edits not exported, Discard of an
  // edited clip), asked in the window.
  // A modal <dialog>: the editor behind it is inert, Tab stays inside, Esc keeps editing, and
  // focus starts on Keep editing so a stray Enter or Space never loses work.

  let { store }: { store: EditorStore } = $props();

  const TEXT: Record<ConfirmKind, { message: string; action: string }> = {
    close: { message: 'Close without exporting? Your edits will be lost.', action: 'Close' },
    move: { message: "Move on? Your edits weren't exported.", action: 'Move on' },
    discard: { message: 'Send this clip to the Recycle Bin?', action: 'Discard' },
  };

  let dialog = $state<HTMLDialogElement>();
  let keep = $state<HTMLButtonElement>();
  const kind = $derived(store.confirm);

  $effect(() => {
    const d = dialog;
    if (!d) return;
    if (kind && !d.open) {
      d.showModal();
      keep?.focus();
    } else if (!kind && d.open) {
      d.close();
    }
  });

  // Esc fires `cancel`; a second quick Esc may close the dialog without one, so `close` answers too.
  function onCancel(e: Event) {
    e.preventDefault();
    store.answer(false);
  }

  function onClose() {
    if (store.confirm) store.answer(false);
  }

  /** Keeps Tab and Shift+Tab cycling between the two buttons. */
  function onKeydown(e: KeyboardEvent) {
    if (e.key !== 'Tab' || !dialog) return;
    const buttons = [...dialog.querySelectorAll<HTMLButtonElement>('button')];
    const first = buttons[0];
    const last = buttons[buttons.length - 1];
    if (e.shiftKey && document.activeElement === first) {
      e.preventDefault();
      last.focus();
    } else if (!e.shiftKey && document.activeElement === last) {
      e.preventDefault();
      first.focus();
    }
  }
</script>

<dialog
  bind:this={dialog}
  role="alertdialog"
  aria-labelledby="confirm-message"
  oncancel={onCancel}
  onclose={onClose}
  onkeydown={onKeydown}
>
  {#if kind}
    <p id="confirm-message">{TEXT[kind].message}</p>
    <div class="actions">
      <button type="button" class="btn cap" onclick={() => store.answer(true)}>{TEXT[kind].action}</button>
      <!-- svelte-ignore a11y_autofocus -->
      <button type="button" class="btn keep cap" autofocus bind:this={keep} onclick={() => store.answer(false)}
        >Keep editing</button
      >
    </div>
  {/if}
</dialog>

<style>
  dialog {
    width: min(400px, calc(100vw - 48px));
    margin: auto;
    padding: 24px;
    background: var(--charcoal);
    color: var(--bone);
    border: 1px solid var(--smoke);
    border-radius: 8px;
    box-shadow: 0 12px 32px rgb(0 0 0 / 0.5);
  }
  dialog::backdrop {
    background: rgb(0 0 0 / 0.6);
  }
  dialog[open] {
    animation: rise 0.16s cubic-bezier(0.16, 1, 0.3, 1);
  }
  @keyframes rise {
    from {
      transform: translateY(6px);
    }
  }
  p {
    margin: 0 0 24px;
    font-size: 14px;
    line-height: 1.45;
    letter-spacing: -0.011em;
    text-wrap: balance;
  }
  .actions {
    display: flex;
    justify-content: flex-end;
    gap: 12px;
  }
  .btn {
    padding: 10px 16px;
    border-radius: 8px;
    border: 1px solid var(--iron);
    background: transparent;
    color: var(--ash);
    transition:
      border-color 0.12s ease-out,
      background-color 0.12s ease-out,
      color 0.12s ease-out;
  }
  .btn:hover {
    border-color: var(--ash);
    color: var(--bone);
    background: var(--smoke);
  }
  .btn:active {
    background: var(--graphite);
  }
  /* The safe answer reads as the one to take. */
  .btn.keep {
    border-color: var(--bone);
    color: var(--bone);
  }

  @media (prefers-reduced-motion: reduce) {
    dialog[open] {
      animation: none;
    }
    .btn {
      transition: none;
    }
  }
</style>
