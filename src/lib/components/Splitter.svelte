<!--
  A drag handle on a column's left or right edge, or a panel's top edge. Reports the distance
  dragged since the drag began (so the owner can clamp without drift); arrow keys nudge by
  8 px; double-click resets.
-->
<script lang="ts">
  type Props = {
    /** Accessible name, e.g. "Resize Author column". */
    label: string;
    /** Which edge of its (relatively positioned) parent it sits on. */
    edge: "left" | "right" | "top";
    /** The current width (height, on the top edge), announced to assistive technology. */
    value: number;
    /** A drag (or key nudge) begins. */
    onstart: () => void;
    /** Moved `d` px since `onstart`: right (down, on the top edge) is positive. */
    onmove: (d: number) => void;
    /** The drag ended; time to save. */
    onend: () => void;
    onreset: () => void;
  };

  let { label, edge, value, onstart, onmove, onend, onreset }: Props = $props();

  /** On the top edge it moves up and down. */
  const across = $derived(edge === "top");

  let start = 0;
  let dragging = $state(false);

  function onPointerDown(e: PointerEvent) {
    if (e.button !== 0) return;
    e.preventDefault();
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    start = across ? e.clientY : e.clientX;
    dragging = true;
    onstart();
  }

  function onPointerMove(e: PointerEvent) {
    if (dragging) onmove((across ? e.clientY : e.clientX) - start);
  }

  function onPointerUp(e: PointerEvent) {
    if (!dragging) return;
    (e.currentTarget as HTMLElement).releasePointerCapture(e.pointerId);
    dragging = false;
    onend();
  }

  function onKeydown(e: KeyboardEvent) {
    const [back, forward] = across ? ["ArrowUp", "ArrowDown"] : ["ArrowLeft", "ArrowRight"];
    const step = e.key === back ? -8 : e.key === forward ? 8 : 0;
    if (step === 0) return;
    e.preventDefault();
    e.stopPropagation();
    onstart();
    onmove(step);
    onend();
  }
</script>

<!-- A focusable separator is a widget (ARIA): it takes pointer and arrow keys. -->
<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
<div
  class="splitter {edge}"
  class:dragging
  role="separator"
  aria-orientation={across ? "horizontal" : "vertical"}
  aria-label={label}
  aria-valuenow={Math.round(value)}
  tabindex="0"
  onpointerdown={onPointerDown}
  onpointermove={onPointerMove}
  onpointerup={onPointerUp}
  onpointercancel={onPointerUp}
  ondblclick={onreset}
  onkeydown={onKeydown}
></div>

<style>
  .splitter {
    position: absolute;
    top: 0;
    bottom: 0;
    z-index: 1;
    width: 9px;
    cursor: col-resize;
    touch-action: none;
  }

  .left {
    left: -5px;
  }

  .right {
    right: -5px;
  }

  .top {
    top: -5px;
    bottom: auto;
    left: 0;
    right: 0;
    width: auto;
    height: 9px;
    cursor: row-resize;
  }

  /* The visible rule: a hairline that firms up on hover and while dragging. */
  .splitter::after {
    content: "";
    position: absolute;
    top: 7px;
    bottom: 7px;
    left: 4px;
    width: 1px;
    background: var(--border);
    transition: background-color 80ms linear;
  }

  .splitter:hover::after,
  .splitter.dragging::after,
  .splitter:focus-visible::after {
    top: 3px;
    bottom: 3px;
    background: var(--accent);
  }

  /* Along a panel's top edge the panel draws its own border; the rule shows only when used. */
  .top::after,
  .top:hover::after,
  .top.dragging::after,
  .top:focus-visible::after {
    top: 4px;
    bottom: auto;
    left: 0;
    right: 0;
    width: auto;
    height: 1px;
  }

  .top::after {
    background: transparent;
  }

  .splitter:focus-visible {
    outline: none;
  }
</style>
