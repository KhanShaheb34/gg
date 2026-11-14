<!--
  The toast stack, bottom right. Info toasts leave after 4 s and errors after 10 s (10 s too
  for any with buttons); hovering the stack, or keyboard focus inside it, holds them all. A persistent one stays until
  dismissed. Its buttons follow the message; a folded detail (git's own text) opens below.
-->
<script lang="ts">
  import { visibleText } from "../format";
  import { app, dismissToast, type Toast, type ToastAction } from "../state/app.svelte";

  const LIFETIME_MS = { info: 4_000, error: 10_000 } as const;
  const WITH_BUTTONS_MS = 10_000;

  let hovered = $state(false);
  let focused = $state(false);
  const held = $derived(hovered || focused);
  let stack: HTMLDivElement;

  // A focused toast that goes (dismissed) takes the focus with it, without a focusout.
  $effect(() => {
    void app.toasts.length;
    if (focused && !stack.contains(document.activeElement)) focused = false;
  });
  /** Toasts whose folded detail is open. */
  let unfolded: Set<number> = $state(new Set());

  /** Removes `toast` once it has been shown, unhovered, for its lifetime. */
  function expire(_node: HTMLElement, toast: Toast) {
    if (toast.persistent) return;
    let left: number = toast.actions?.length ? WITH_BUTTONS_MS : LIFETIME_MS[toast.kind];
    let started = Date.now();
    let timer: ReturnType<typeof setTimeout> | null = null;
    const run = () => {
      started = Date.now();
      timer = setTimeout(() => dismissToast(toast.id), left);
    };
    const pause = () => {
      if (timer === null) return;
      clearTimeout(timer);
      timer = null;
      left -= Date.now() - started;
    };
    $effect(() => {
      if (held) pause();
      else run();
      return pause;
    });
  }

  function press(toast: Toast, action: ToastAction) {
    if (!action.keepOpen) dismissToast(toast.id);
    void action.run();
  }

  function toggleDetail(id: number) {
    const next = new Set(unfolded);
    if (!next.delete(id)) next.add(id);
    unfolded = next;
  }
</script>

<div
  class="stack"
  bind:this={stack}
  role="region"
  aria-label="Notifications"
  onmouseenter={() => (hovered = true)}
  onmouseleave={() => (hovered = false)}
  onfocusin={() => (focused = true)}
  onfocusout={(e) => (focused = e.currentTarget.contains(e.relatedTarget as Node | null))}
>
  {#each app.toasts as toast (toast.id)}
    <div class="toast {toast.kind}" role={toast.kind === "error" ? "alert" : "status"} use:expire={toast}>
      <span class="icon" aria-hidden="true">
        {#if toast.kind === "error"}
          <svg width="16" height="16" viewBox="0 0 16 16">
            <circle cx="8" cy="8" r="6.5" fill="currentColor" />
            <path d="M8 4.6v4.2" stroke="var(--bg-elevated)" stroke-width="1.6" stroke-linecap="round" />
            <circle cx="8" cy="11.2" r="0.95" fill="var(--bg-elevated)" />
          </svg>
        {:else}
          <svg width="16" height="16" viewBox="0 0 16 16">
            <circle cx="8" cy="8" r="6.5" fill="currentColor" />
            <path d="M8 7.2v4.2" stroke="var(--bg-elevated)" stroke-width="1.6" stroke-linecap="round" />
            <circle cx="8" cy="4.8" r="0.95" fill="var(--bg-elevated)" />
          </svg>
        {/if}
      </span>
      <div class="body selectable">
        <!-- Messages carry git's text (paths, branch names): isolated, so it cannot reorder
             what is around it. -->
        <p class="message"><bdi>{visibleText(toast.message)}</bdi></p>
        {#if toast.detail && !toast.foldDetail}
          <p class="detail mono"><bdi>{toast.detail}</bdi></p>
        {/if}
        {#if toast.actions?.length || toast.link || (toast.detail && toast.foldDetail)}
          <div class="row">
            {#each toast.actions ?? [] as action, i (i)}
              <button class="action" class:danger={action.danger} onclick={() => press(toast, action)}
                >{action.label}</button
              >
            {/each}
            {#if toast.detail && toast.foldDetail}
              <button
                class="link"
                aria-expanded={unfolded.has(toast.id)}
                onclick={() => toggleDetail(toast.id)}
              >
                <svg class="chevron" width="8" height="8" viewBox="0 0 8 8" aria-hidden="true">
                  <path d="M2.5 1.5 5.5 4l-3 2.5" fill="none" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" stroke-linejoin="round" />
                </svg>
                Git output
              </button>
            {/if}
            {#if toast.link}
              {@const link = toast.link}
              <button class="link" onclick={() => press(toast, link)}>{link.label}</button>
            {/if}
          </div>
          {#if toast.detail && toast.foldDetail && unfolded.has(toast.id)}
            <pre class="output selectable">{toast.detail}</pre>
          {/if}
        {/if}
      </div>
      <button class="dismiss" aria-label="Dismiss notification" onclick={() => dismissToast(toast.id)}>
        <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden="true">
          <path d="M2 2l6 6M8 2L2 8" stroke="currentColor" stroke-width="1.4" stroke-linecap="round" />
        </svg>
      </button>
    </div>
  {/each}
</div>

<style>
  .stack {
    position: fixed;
    right: 16px;
    bottom: 16px;
    z-index: 50;
    display: flex;
    flex-direction: column;
    align-items: flex-end;
    gap: 8px;
    width: min(380px, calc(100vw - 32px));
    pointer-events: none;
  }

  .toast {
    display: grid;
    grid-template-columns: 16px minmax(0, 1fr) 20px;
    align-items: start;
    gap: 10px;
    width: 100%;
    padding: 10px 10px 10px 12px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
    background: var(--bg-elevated);
    box-shadow: var(--shadow-pop);
    pointer-events: auto;
    animation: enter 180ms var(--ease);
  }

  .icon {
    padding-top: 1px;
    color: var(--accent);
  }

  .error .icon {
    color: var(--danger);
  }

  .icon svg {
    display: block;
  }

  .message {
    overflow-wrap: anywhere;
    line-height: 18px;
  }

  .detail {
    display: -webkit-box;
    margin-top: 4px;
    overflow: hidden;
    color: var(--text-muted);
    font-size: var(--fs-xs);
    line-height: 1.45;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    -webkit-box-orient: vertical;
    -webkit-line-clamp: 4;
    line-clamp: 4;
  }

  .row {
    display: flex;
    flex-wrap: wrap;
    align-items: center;
    gap: 6px;
    margin-top: 8px;
  }

  .action {
    padding: 3px 10px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg);
    font-size: var(--fs-sm);
    font-weight: 500;
  }

  .action:hover {
    background: var(--bg-hover);
  }

  .action:active {
    background: var(--bg-pressed);
  }

  .action.danger {
    border-color: color-mix(in srgb, var(--danger) 45%, var(--border-strong));
    color: var(--danger);
  }

  .action.danger:hover {
    background: var(--danger-tint);
  }

  /* Quieter than a button: text in the accent colour. */
  .link {
    display: inline-flex;
    align-items: center;
    gap: 4px;
    padding: 3px 4px;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: var(--accent);
    font-size: var(--fs-sm);
    font-weight: 500;
  }

  .link:hover {
    background: var(--bg-hover);
  }

  .chevron {
    transition: transform 120ms var(--ease);
  }

  .link[aria-expanded="true"] .chevron {
    transform: rotate(90deg);
  }

  .output {
    max-height: 180px;
    margin: 6px 0 0;
    padding: 7px 9px;
    overflow: auto;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--field-bg);
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    line-height: 1.45;
    white-space: pre-wrap;
    overflow-wrap: anywhere;
    unicode-bidi: isolate;
  }

  .dismiss {
    display: grid;
    place-items: center;
    width: 20px;
    height: 20px;
    padding: 0;
    border: 0;
    border-radius: 4px;
    background: transparent;
    color: var(--text-faint);
  }

  .dismiss:hover {
    background: var(--bg-hover);
    color: var(--text);
  }

  .dismiss:active {
    background: var(--bg-pressed);
  }

  @keyframes enter {
    from {
      opacity: 0;
      transform: translateY(6px);
    }
  }
</style>
