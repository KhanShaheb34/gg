<!--
  One line of text that, when it does not fit, is cut in the middle: its start gives way and
  its last characters stay, so "feature/payments-service-v2" reads "feature/pay…service-v2"
  and a folder or file name keeps its end. The whole text stays in the element (it sizes it,
  and is what is copied and read out); the cut text is painted over it, measured to fit the
  room there is, whenever that room changes. Control and bidi characters are made visible
  (visibleText); the tooltip has the text as it is.
-->
<script module lang="ts">
  /** One observer for every cut text; each element's own callback. */
  const callbacks = new WeakMap<Element, () => void>();
  let observer: ResizeObserver | null = null;
  let canvas: CanvasRenderingContext2D | null = null;

  function observe(el: Element, callback: () => void): () => void {
    observer ??= new ResizeObserver((entries) => {
      for (const entry of entries) callbacks.get(entry.target)?.();
    });
    callbacks.set(el, callback);
    observer.observe(el);
    return () => {
      callbacks.delete(el);
      observer?.unobserve(el);
    };
  }

  /** Measures strings in `font`, px. */
  function measurer(font: string): (text: string) => number {
    canvas ??= document.createElement("canvas").getContext("2d");
    const ctx = canvas;
    if (!ctx) return (text) => text.length * 7;
    return (text) => {
      ctx.font = font;
      return ctx.measureText(text).width;
    };
  }
</script>

<script lang="ts">
  import { cutToFit, visibleText } from "../format";

  type Props = {
    text: string;
    /** Characters at the end that always stay (at most half the text). */
    keep?: number;
    /** The tooltip; the text itself by default; null for none. */
    title?: string | null;
    class?: string;
  };

  let { text, keep = 10, title = undefined, class: className = "" }: Props = $props();

  const visible = $derived(visibleText(text));
  let el: HTMLElement | undefined = $state();
  /** What is painted when the whole text does not fit; null when it does. */
  let shown: string | null = $state(null);

  function fit() {
    if (!el) return;
    // A pixel of slack: the canvas and the page round differently.
    const room = el.clientWidth - 1;
    shown = el.scrollWidth <= el.clientWidth ? null : cutToFit(visible, room, keep, measurer(getComputedStyle(el).font));
  }

  $effect(() => {
    if (!el) return;
    void visible;
    void keep;
    fit();
    return observe(el, fit);
  });
</script>

<bdi
  class="mid {className}"
  class:cut={shown !== null}
  data-shown={shown}
  title={title === undefined ? text : (title ?? undefined)}
  bind:this={el}>{visible}</bdi
>

<style>
  .mid {
    position: relative;
    display: inline-block;
    min-width: 0;
    max-width: 100%;
    overflow: hidden;
    white-space: pre;
    vertical-align: bottom;
  }

  /* The whole text keeps its place, unpainted; the cut one is painted over it. */
  .cut {
    -webkit-text-fill-color: transparent;
  }

  .cut::after {
    content: attr(data-shown);
    position: absolute;
    top: 0;
    left: 0;
    white-space: pre;
    -webkit-text-fill-color: currentColor;
  }
</style>
