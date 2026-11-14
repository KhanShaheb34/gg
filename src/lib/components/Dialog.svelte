<!--
  The frame of every modal dialog: a title, the dialog's own content, and Cancel and a
  primary button. Modal: the window behind is dimmed and takes no input; Tab stays inside;
  focus goes back where it was when the dialog closes. Escape (and Cancel) cancel; Enter
  submits, unless focus is on Cancel or in a multi-line field (Cmd/Ctrl+Enter submits
  there), or the primary button is disabled (`valid` false). A dangerous primary button is
  red and never has the focus to begin with: the first field has it, else Cancel. Enter on a
  button or link inside the dialog presses it instead. A dangerous dialog (an alertdialog)
  is described by its content, its message.

  Submitting happens once per dialog (`once`; a dialog that stays open after a refusal
  passes false and guards itself while its request is out). A held Enter does not repeat
  it. A dangerous primary button takes no click or Enter in its first ARM_MS: a second
  click meant for the dialog before it must not confirm one that opened in its place.
-->
<script lang="ts">
  import { onMount, type Snippet } from "svelte";

  type Props = {
    title: string;
    /** The primary button's label: what it does ("Delete branch"). */
    primary: string;
    /** The primary action destroys something. */
    danger?: boolean;
    /** The fields are valid: the primary button is enabled. */
    valid?: boolean;
    cancelLabel?: string;
    /** Submits at most once (default). */
    once?: boolean;
    onSubmit: () => void;
    onCancel: () => void;
    children: Snippet;
  };

  let {
    title,
    primary,
    danger = false,
    valid = true,
    cancelLabel = "Cancel",
    once = true,
    onSubmit,
    onCancel,
    children,
  }: Props = $props();

  /** How long a dangerous primary button ignores activation after the dialog opens, ms. */
  const ARM_MS = 400;
  const openedAt = performance.now();
  let submitted = false;

  const uid = Math.random().toString(36).slice(2, 8);
  let box: HTMLDivElement;
  let cancelButton: HTMLButtonElement;
  let primaryButton: HTMLButtonElement;

  const FOCUSABLE =
    'button:not([disabled]), input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [href], [tabindex]:not([tabindex="-1"])';

  function submit() {
    if (!valid || (once && submitted)) return;
    if (danger && performance.now() - openedAt < ARM_MS) return;
    submitted = true;
    onSubmit();
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.isComposing) return;
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      onCancel();
      return;
    }
    if (e.key === "Tab") {
      const items = [...box.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => el.offsetParent !== null);
      if (items.length === 0) return;
      const at = items.indexOf(document.activeElement as HTMLElement);
      const next = e.shiftKey ? (at <= 0 ? items.length - 1 : at - 1) : at === items.length - 1 ? 0 : at + 1;
      e.preventDefault();
      items[next].focus();
      return;
    }
    if (e.key === "Enter" && !e.altKey && !e.shiftKey) {
      if (e.repeat) {
        e.preventDefault();
        return;
      }
      const target = e.target as HTMLElement;
      // A button (Cancel, or one of the content's) or a link does what it does itself.
      if (target !== primaryButton && target.closest("button, a[href], summary, [role=button]")) return;
      if (target instanceof HTMLTextAreaElement && !(e.metaKey || e.ctrlKey)) return;
      e.preventDefault();
      submit();
    }
  }

  onMount(() => {
    const before = document.activeElement;
    // A marked field first (a list matches in document order, so it is asked for alone).
    const field =
      box.querySelector<HTMLElement>("[data-autofocus]") ??
      box.querySelector<HTMLElement>(
        "input:not([disabled]):not([type=hidden]), select:not([disabled]), textarea:not([disabled])",
      );
    const first = field ?? (danger || !valid ? cancelButton : primaryButton);
    first.focus();
    return () => {
      if (before instanceof HTMLElement && before.isConnected && before !== document.body) {
        before.focus({ preventScroll: true });
      }
    };
  });
</script>

<!-- The backdrop takes the pointer so nothing behind reacts, and keeps focus in the dialog. -->
<div class="backdrop" role="presentation" onmousedown={(e) => e.target === e.currentTarget && e.preventDefault()}>
  <div
    class="dialog"
    role={danger ? "alertdialog" : "dialog"}
    aria-modal="true"
    aria-labelledby="dialog-title-{uid}"
    aria-describedby={danger ? `dialog-body-${uid}` : undefined}
    tabindex="-1"
    bind:this={box}
    onkeydown={onKeydown}
  >
    <h2 class="title" id="dialog-title-{uid}">{title}</h2>
    <div class="body" id="dialog-body-{uid}">
      {@render children()}
    </div>
    <div class="buttons">
      <button class="button" bind:this={cancelButton} onclick={onCancel}>{cancelLabel}</button>
      <button class="button primary" class:danger disabled={!valid} bind:this={primaryButton} onclick={submit}
        >{primary}</button
      >
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
    align-items: flex-start;
    padding: max(56px, 14vh) 16px 16px;
    background: var(--scrim);
    animation: fade 120ms linear;
  }

  .dialog {
    display: flex;
    flex-direction: column;
    width: min(460px, 100%);
    max-height: calc(100vh - 80px);
    border: 1px solid var(--border-strong);
    border-radius: 12px;
    background: var(--bg-elevated);
    box-shadow: var(--shadow-dialog);
    outline: none;
    animation: rise 160ms var(--ease);
  }

  .title {
    padding: 16px 18px 0;
    font-size: 14px;
    font-weight: 650;
    line-height: 1.35;
    overflow-wrap: anywhere;
  }

  .body {
    min-height: 0;
    overflow-y: auto;
    padding: 10px 18px 4px;
  }

  .buttons {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 14px 18px 16px;
  }

  .button {
    min-width: 76px;
    height: 28px;
    padding: 0 14px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg);
    color: var(--text);
    font-size: var(--fs);
    font-weight: 500;
    transition:
      background-color 80ms linear,
      border-color 80ms linear;
  }

  .button:hover {
    background: var(--bg-hover);
  }

  .button:active {
    background: var(--bg-pressed);
  }

  .button:focus-visible {
    outline: 2px solid var(--focus-ring);
    outline-offset: 2px;
  }

  .primary {
    border-color: var(--accent);
    background: var(--accent);
    color: var(--accent-contrast);
  }

  .primary:hover {
    border-color: var(--accent-hover);
    background: var(--accent-hover);
  }

  .primary:active {
    background: color-mix(in srgb, var(--accent-hover) 88%, black);
  }

  .primary.danger {
    border-color: var(--danger-fill);
    background: var(--danger-fill);
    color: var(--danger-contrast);
  }

  .primary.danger:hover {
    border-color: var(--danger-hover);
    background: var(--danger-hover);
  }

  .primary.danger:focus-visible {
    outline-color: color-mix(in srgb, var(--danger) 55%, transparent);
  }

  .primary:disabled {
    border-color: var(--border-strong);
    background: var(--bg-pressed);
    color: var(--text-faint);
  }

  @keyframes fade {
    from {
      opacity: 0;
    }
  }

  @keyframes rise {
    from {
      opacity: 0;
      transform: translateY(-6px) scale(0.985);
    }
  }
</style>
