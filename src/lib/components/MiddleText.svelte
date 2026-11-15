<!--
  One line of text that, when it does not fit, is cut in the middle: its start gives way and
  its last characters stay, so "feature/payments-service-v2" reads "feature/pay…service-v2"
  and a folder or file name keeps its end. The whole text stays in the element (it sizes it,
  and is what is copied and read out); the cut text is painted over it, measured to fit the
  room there is, whenever that room changes. With `dimThrough` ("/"), what comes before its last
  occurrence (a file's folders) is quieter than the rest (its name). Control and bidi
  characters are made visible (visibleText); the tooltip has the text as it is.
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
    /** What comes up to the last of these characters is quieter (a path's folders). */
    dimThrough?: string;
    class?: string;
  };

  let { text, keep = 10, title = undefined, dimThrough, class: className = "" }: Props = $props();

  /** `s` split where its quiet part ends. */
  const split = (s: string): [string, string] => {
    const at = dimThrough === undefined ? -1 : s.lastIndexOf(dimThrough);
    return [s.slice(0, at + 1), s.slice(at + 1)];
  };

  const visible = $derived(visibleText(text));
  let el: HTMLElement | undefined = $state();
  /** What is painted when the whole text does not fit (its quiet part, the rest, and how wide the quiet part is); null when it fits. */
  let shown: { dim: string; rest: string; dimW: number } | null = $state(null);
  const parts = $derived(split(visible));

  function fit() {
    if (!el) return;
    // A pixel of slack: the canvas and the page round differently.
    const room = el.clientWidth - 1;
    if (el.scrollWidth <= el.clientWidth) {
      shown = null;
      return;
    }
    const width = measurer(getComputedStyle(el).font);
    const cut = cutToFit(visible, room, keep, width);
    if (cut === null) {
      shown = null;
      return;
    }
    const [dim, rest] = split(cut);
    shown = { dim, rest, dimW: width(dim) };
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
  data-dim={shown?.dim}
  data-shown={shown?.rest}
  style:--dim-w={shown ? `${shown.dimW}px` : undefined}
  title={title === undefined ? text : (title ?? undefined)}
  bind:this={el}
  >{#if parts[0]}<span class="dim">{parts[0]}</span>{/if}{parts[1]}</bdi
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

  .cut::before,
  .cut::after {
    position: absolute;
    top: 0;
    white-space: pre;
    -webkit-text-fill-color: currentColor;
  }

  .cut::before {
    content: attr(data-dim);
    left: 0;
    color: var(--text-muted);
  }

  .cut::after {
    content: attr(data-shown);
    left: var(--dim-w, 0);
  }

  .dim {
    color: var(--text-muted);
  }
</style>
