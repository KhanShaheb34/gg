<!--
  A tall modal sheet over the window (Settings, Keyboard shortcuts): a title bar with a close
  button, and content that scrolls. Modal as a dialog is: the window behind is dimmed and
  takes no input, Tab stays inside, Escape closes it (unless something inside handled the key),
  and the focus goes back where it was when it closes.
-->
<script lang="ts">
  import { onMount, type Snippet } from "svelte";

  type Props = {
    title: string;
    /** Widest it gets, px. */
    width?: number;
    onClose: () => void;
    /** Beside the title (a status line). */
    aside?: Snippet;
    children: Snippet;
    testid?: string;
  };

  let { title, width = 760, onClose, aside, children, testid }: Props = $props();

  const uid = Math.random().toString(36).slice(2, 8);
  let box: HTMLDivElement;

  const FOCUSABLE =
    'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [href], [tabindex]:not([tabindex="-1"])';

  function onKeydown(e: KeyboardEvent) {
    if (e.isComposing) return;
    if (e.key === "Escape" && !e.defaultPrevented) {
      e.preventDefault();
      e.stopPropagation();
      onClose();
      return;
    }
    if (e.key === "Tab") {
      const items = [...box.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => el.offsetParent !== null);
      if (items.length === 0) return;
      const at = items.indexOf(document.activeElement as HTMLElement);
      const next = e.shiftKey ? (at <= 0 ? items.length - 1 : at - 1) : at === items.length - 1 ? 0 : at + 1;
      e.preventDefault();
      items[next].focus();
    }
  }

  onMount(() => {
    const before = document.activeElement;
    (box.querySelector<HTMLElement>("[data-autofocus]") ?? box).focus();
    return () => {
      const now = document.activeElement;
      const here = now === null || now === document.body || box.contains(now);
      if (here && before instanceof HTMLElement && before.isConnected && before !== document.body) {
        before.focus({ preventScroll: true });
      }
    };
  });
</script>

<div class="backdrop" role="presentation" onmousedown={(e) => e.target === e.currentTarget && e.preventDefault()}>
  <div
    class="sheet"
    style:--sheet-w="{width}px"
    role="dialog"
    aria-modal="true"
    aria-labelledby="sheet-title-{uid}"
    tabindex="-1"
    data-testid={testid}
    bind:this={box}
    onkeydown={onKeydown}
  >
    <header class="head">
      <h2 class="title" id="sheet-title-{uid}">{title}</h2>
      {#if aside}{@render aside()}{/if}
      <button class="icon-btn close" aria-label="Close" title="Close (Esc)" onclick={onClose}>
        <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
          <path d="M2.5 2.5l7 7M9.5 2.5l-7 7" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
        </svg>
      </button>
    </header>
    <div class="content">
      {@render children()}
    </div>
  </div>
</div>

<style>
  .backdrop {
    position: fixed;
    inset: 0;
    z-index: 60;
    display: flex;
    justify-content: center;
    align-items: stretch;
    padding: calc(var(--titlebar-h) + 12px) 16px 16px;
    background: var(--scrim);
    animation: fade 120ms linear;
  }

  .sheet {
    display: flex;
    flex-direction: column;
    width: min(var(--sheet-w), 100%);
    max-height: 100%;
    border: 1px solid var(--border-strong);
    border-radius: 12px;
    background: var(--bg-elevated);
    box-shadow: var(--shadow-dialog);
    outline: none;
    overflow: hidden;
    animation: rise 160ms var(--ease);
  }

  .head {
    display: flex;
    align-items: center;
    gap: 10px;
    flex: none;
    height: 46px;
    padding: 0 10px 0 18px;
    border-bottom: 1px solid var(--border);
  }

  .title {
    flex: 1 1 auto;
    min-width: 0;
    font-size: 14px;
    font-weight: 650;
  }

  .close {
    flex: none;
  }

  .content {
    display: flex;
    flex: 1 1 auto;
    min-height: 0;
  }

  @keyframes fade {
    from {
      opacity: 0;
    }
  }

  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(6px) scale(0.99);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .backdrop,
    .sheet {
      animation: none;
    }
  }
</style>
