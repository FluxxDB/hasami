<script lang="ts">
  // The row under the top bar: an outcome that needs words (topbar.ts `outcomeOf`): why an export
  // or a discard failed, and that the original is untouched; a saved export's note; or Export
  // audio's saved file. It stays until dismissed, the next export of the clip, or the next clip.

  import { fileName } from '../../display';
  import type { Result } from '../../exportLog.svelte';
  import type { EditorStore } from '../../store.svelte';
  import { KEPT, LABEL } from './topbar';

  let { store, outcome }: { store: EditorStore; outcome: Result | null } = $props();

  // Tells one outcome from the next, so a dismissed one stays dismissed and a new one shows.
  const outcomeKey = $derived(
    outcome
      ? `${store.visit}:${outcome.action}:${outcome.state}:${outcome.output ?? outcome.message}:${outcome.note ?? ''}:${outcome.action === 'discard' ? store.discardFailure?.seq : ''}`
      : '',
  );
  let dismissed = $state('');
  const show = $derived(!!outcome && outcomeKey !== dismissed);
</script>

{#if show && outcome}
  <div class="outcome" class:failed={outcome.state === 'failed'}>
    {#if outcome.state === 'failed'}
      <span class="cap plabel">{LABEL[outcome.action]} failed</span>
      <span class="msg">{outcome.message}</span>
      {#if outcome.originalIntact}<span class="kept">{KEPT}</span>{/if}
    {:else if outcome.note}
      <!-- The note names the saved file itself ("Saved …, but couldn't make the 20 MB copy: …"). -->
      <span class="cap plabel">{LABEL[outcome.action]}</span>
      <span class="msg" title={outcome.note}>{outcome.note}</span>
    {:else}
      <span class="cap plabel">Saved</span><span class="out" title={outcome.output}>{fileName(outcome.output ?? '')}</span>
    {/if}
    <button type="button" class="ctl dismiss" aria-label="Dismiss" onclick={() => (dismissed = outcomeKey)}>
      <svg width="10" height="10" viewBox="0 0 12 12" aria-hidden="true"
        ><path d="M2 2l8 8M10 2l-8 8" stroke="currentColor" fill="none" /></svg
      >
    </button>
  </div>
{/if}

<style>
  /* One line of plain words, quiet but full width. */
  .outcome {
    flex: none;
    height: 32px;
    display: flex;
    align-items: center;
    gap: 12px;
    padding: 0 10px 0 18px;
    border-bottom: 1px solid var(--line);
    background: var(--void);
    white-space: nowrap;
  }
  .plabel {
    color: var(--pebble);
    flex: none;
  }
  .out,
  .msg {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }
  .msg {
    flex: 0 1 auto;
  }
  .kept {
    flex: none;
    color: var(--pebble);
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
  .ctl {
    width: 34px;
    height: 28px;
    display: grid;
    place-items: center;
    color: var(--pebble);
    border-radius: 3px;
  }
  .ctl:hover {
    background: var(--charcoal);
    color: var(--bone);
  }
  .dismiss {
    margin-left: auto;
    height: 24px;
  }

  @media (max-width: 1180px) {
    .ctl {
      width: 30px;
    }
  }
  @media (max-width: 1100px) {
    .ctl {
      width: 28px;
    }
  }

  @media (prefers-reduced-motion: reduce) {
    button {
      transition: none;
    }
  }
</style>
