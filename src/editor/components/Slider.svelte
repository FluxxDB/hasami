<script lang="ts">
  // A hairline slider: a 1px rail, an ash fill up to the knob, and a bone knob. A native range
  // input underneath carries the value and its accessible name.
  interface Props {
    /** 0..1 */
    value: number;
    label: string;
    valuetext: string;
    disabled?: boolean;
    oninput?: (value: number) => void;
  }

  let { value, label, valuetext, disabled = false, oninput }: Props = $props();
</script>

<div class="slider" style:--pos={value}>
  <div class="rail" aria-hidden="true"><div class="fill"></div></div>
  <input
    type="range"
    min="0"
    max="1"
    step="0.001"
    {value}
    {disabled}
    aria-label={label}
    aria-valuetext={valuetext}
    oninput={(e) => oninput?.(Number(e.currentTarget.value))}
  />
</div>

<style>
  .slider {
    position: relative;
    flex: 1;
    min-width: 40px;
    height: 16px;
    --knob: 11px;
  }
  .rail {
    position: absolute;
    left: 0;
    right: 0;
    top: 50%;
    height: 1px;
    background: var(--iron);
  }
  /* Ends under the knob's centre, which travels from knob/2 to 100% − knob/2. */
  .fill {
    height: 1px;
    width: calc(var(--knob) / 2 + var(--pos) * (100% - var(--knob)));
    background: var(--ash);
  }
  input {
    position: absolute;
    inset: 0;
    width: 100%;
    height: 100%;
    margin: 0;
    background: transparent;
    appearance: none;
    cursor: pointer;
  }
  input:disabled {
    cursor: default;
  }
  input::-webkit-slider-runnable-track {
    height: 16px;
    background: transparent;
  }
  input::-webkit-slider-thumb {
    appearance: none;
    width: var(--knob);
    height: var(--knob);
    margin-top: calc((16px - var(--knob)) / 2);
    border-radius: 50%;
    background: var(--bone);
  }
  input:focus-visible {
    outline: none;
  }
  input:focus-visible::-webkit-slider-thumb {
    outline: 1.5px solid var(--bone);
    outline-offset: 2px;
  }
</style>
