<script lang="ts">
  // Minimize, maximize and close: the window has no system title bar (the top bar is its drag
  // region). Closing with unexported edits asks first (main.ts).

  interface Props {
    onMinimize: () => void;
    onToggleMaximize: () => void;
    onClose: () => void;
  }

  let { onMinimize, onToggleMaximize, onClose }: Props = $props();
</script>

<div class="winctl">
  <button type="button" class="ctl" aria-label="Minimize" onclick={onMinimize}>
    <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"><path d="M1 6h10" stroke="currentColor" fill="none" /></svg>
  </button>
  <button type="button" class="ctl" aria-label="Maximize" onclick={onToggleMaximize}>
    <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"
      ><rect x="1.5" y="1.5" width="9" height="9" fill="none" stroke="currentColor" /></svg
    >
  </button>
  <button type="button" class="ctl close" aria-label="Close" onclick={onClose}>
    <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true"
      ><path d="M2 2l8 8M10 2l-8 8" stroke="currentColor" fill="none" /></svg
    >
  </button>
</div>

<style>
  .winctl {
    flex: none;
    display: flex;
    gap: 2px;
    margin-left: 6px;
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

  @media (max-width: 1440px) {
    .winctl {
      margin-left: 0;
    }
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
