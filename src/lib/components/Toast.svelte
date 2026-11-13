<!--
  The toast stack, bottom right. Info toasts leave after 4 s and errors after 10 s; hovering
  the stack holds them all. A persistent one stays until dismissed; one with an action shows
  it as a button.
-->
<script lang="ts">
  import { app, dismissToast, type Toast } from "../state/app.svelte";

  const LIFETIME_MS = { info: 4_000, error: 10_000 } as const;

  let held = $state(false);

  /** Removes `toast` once it has been shown, unhovered, for its lifetime. */
  function expire(_node: HTMLElement, toast: Toast) {
    if (toast.persistent) return;
    let left: number = LIFETIME_MS[toast.kind];
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
</script>

<div
  class="stack"
  role="region"
  aria-label="Notifications"
  onmouseenter={() => (held = true)}
  onmouseleave={() => (held = false)}
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
        <p class="message"><bdi>{toast.message}</bdi></p>
        {#if toast.detail}
          <p class="detail mono"><bdi>{toast.detail}</bdi></p>
        {/if}
        {#if toast.action}
          {@const action = toast.action}
          <button class="action" onclick={() => action.run()}>{action.label}</button>
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

  .action {
    margin-top: 8px;
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
