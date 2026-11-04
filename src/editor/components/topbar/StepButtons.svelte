<script lang="ts">
  // Prev and Next video: always here; where each goes is said in its tooltip. Next goes to the next
  // newer clip, Prev to the next older one (navigation.ts). aria-disabled rather than disabled, so
  // "No older video in this folder" still shows on hover and focus.

  import type { EditorStore } from '../../store.svelte';
  import { stepTitle } from './topbar';

  let { store }: { store: EditorStore } = $props();

  const nextOk = $derived(!store.busy && store.canNext);
  const prevOk = $derived(!store.busy && store.canPrev);
  const nextTitle = $derived(stepTitle('next', store.nextTarget, !!store.currentPath));
  const prevTitle = $derived(stepTitle('prev', store.prevTarget, !!store.currentPath));
</script>

{#snippet forward()}
  <svg
    width="11"
    height="11"
    viewBox="0 0 12 12"
    fill="none"
    stroke="currentColor"
    stroke-width="1.2"
    aria-hidden="true"
  >
    <path d="M1.5 6h9M7 2.5 10.5 6 7 9.5" />
  </svg>
{/snippet}

{#snippet backward()}
  <svg
    width="11"
    height="11"
    viewBox="0 0 12 12"
    fill="none"
    stroke="currentColor"
    stroke-width="1.2"
    aria-hidden="true"
  >
    <path d="M10.5 6h-9M5 2.5 1.5 6 5 9.5" />
  </svg>
{/snippet}

<div class="steps">
  <button
    type="button"
    class="btn prev cap"
    aria-disabled={!prevOk}
    aria-label="Previous video"
    title={prevTitle}
    onclick={() => prevOk && store.prevVideo()}>{@render backward()} <span class="word">Prev</span></button
  >
  <button
    type="button"
    class="btn next cap"
    aria-disabled={!nextOk}
    aria-label="Next video"
    title={nextTitle}
    onclick={() => nextOk && store.nextVideo()}
    ><span class="word">Next<span class="more">&nbsp;video</span></span> {@render forward()}</button
  >
</div>

<style>
  .steps {
    flex: none;
    display: flex;
    gap: 6px;
  }

  button {
    flex: none;
    white-space: nowrap;
    color: var(--bone);
    background: transparent;
    border: 0;
    padding: 0;
    transition:
      border-color 0.12s ease-out,
      background-color 0.12s ease-out,
      color 0.12s ease-out,
      text-decoration-color 0.12s ease-out;
  }
  button[aria-disabled='true'] {
    opacity: 0.4;
    cursor: default;
  }

  /* 8px outline buttons. */
  .btn {
    display: inline-flex;
    align-items: center;
    gap: 10px;
    padding: 10px 16px;
    border-radius: 8px;
    border: 1px solid var(--ash);
  }
  .btn:hover:not(:disabled, [aria-disabled='true']) {
    border-color: var(--bone);
    background: var(--charcoal);
  }
  .btn:active:not(:disabled, [aria-disabled='true']) {
    background: var(--smoke);
  }

  @media (max-width: 1440px) {
    .btn {
      padding: 10px 12px;
      gap: 8px;
    }
  }

  /* Next video's second word goes first (its tooltip names where it goes). */
  @media (max-width: 1360px) {
    .next .more {
      display: none;
    }
  }

  /* Near the 960px minimum, Prev and Next become a pair of arrows; their names stay in
     aria-label and the tooltip. */
  @media (max-width: 1100px) {
    .btn {
      padding: 10px 10px;
    }
    .steps .word {
      display: none;
    }
    .steps .btn {
      padding: 10px 11px;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    button {
      transition: none;
    }
  }
</style>
