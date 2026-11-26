<!--
  A box to write to a pull request in the Pull request tab: a reply in a review thread, or a
  comment at the end of the conversation. Quiet while it is not used: one line ("Reply…"). It
  grows when focused, and with what is typed (up to about a dozen lines, then it scrolls), and
  its Send button shows. It stays open while it holds a draft.

  Nothing is sent but on Send, or ⌘↩ (Ctrl+Enter) in the box; Send is off while the box is
  blank and while it sends. While it sends the box is read-only and the button says so; a
  failure stays under the box, in the tab's error style, with Show log, and the text stays.
  `lead` goes at the start of the row of the button (a thread's Resolve).
-->
<script lang="ts">
  import type { Snippet } from "svelte";
  import { isMac } from "../platform";
  import type { Draft } from "../state/prCompose.svelte";

  type Props = {
    draft: Draft;
    /** The one line it shows while not used ("Reply…"). */
    placeholder: string;
    /** What the box is, for assistive technology ("Reply to ada on src/a.ts"). */
    label: string;
    /** The button's word ("Reply", "Comment"). */
    sendLabel: string;
    onsend: () => void;
    onlog: (from: number) => void;
    testid: string;
    lead?: Snippet;
  };

  let { draft, placeholder, label, sendLabel, onsend, onlog, testid, lead }: Props = $props();

  const SEND_KEYS = isMac() ? "⌘↩" : "Ctrl+Enter";
  /** Heights of the box: one line, and the least and most once open. */
  const ONE_LINE = 28;
  const OPEN_MIN = 64;
  const OPEN_MAX = 232;

  let field: HTMLTextAreaElement | undefined = $state();
  let focused = $state(false);
  const open = $derived(focused || draft.text !== "" || draft.sending || draft.error !== null);
  const canSend = $derived(!draft.sending && draft.text.trim() !== "");

  // As tall as what it holds (between its least and most), from where it is, so the change
  // eases rather than jumps.
  $effect(() => {
    void draft.text;
    const el = field;
    if (!el) return;
    const want = open ? Math.min(OPEN_MAX, Math.max(OPEN_MIN, natural(el))) : ONE_LINE;
    el.style.height = `${want}px`;
    el.style.overflowY = open && natural(el) > OPEN_MAX ? "auto" : "hidden";
  });

  /** The height `el` would take to show all it holds. */
  function natural(el: HTMLTextAreaElement): number {
    const was = el.style.height;
    el.style.height = "0px";
    const h = el.scrollHeight + 2; // its borders
    el.style.height = was;
    // Read once more, so the height set next eases from where it was.
    void el.offsetHeight;
    return h;
  }

  function send() {
    if (canSend) onsend();
  }

  function onKeydown(e: KeyboardEvent) {
    if (e.isComposing) return;
    if (e.key === "Enter" && (isMac() ? e.metaKey : e.ctrlKey)) {
      e.preventDefault();
      send();
    } else if (e.key === "Escape") {
      // Leaves the box (the draft stays); the panel is not closed by it.
      e.stopPropagation();
      field?.blur();
    }
  }
</script>

{#snippet spinner()}
  <svg class="spin" width="11" height="11" viewBox="0 0 16 16" aria-hidden="true">
    <circle cx="8" cy="8" r="5.6" fill="none" stroke="currentColor" stroke-opacity="0.3" stroke-width="2" />
    <path d="M8 2.4a5.6 5.6 0 0 1 5.6 5.6" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" />
  </svg>
{/snippet}

<div class="composer" class:open data-testid={testid}>
  <textarea
    bind:this={field}
    bind:value={draft.text}
    class="field"
    class:sending={draft.sending}
    rows="1"
    {placeholder}
    aria-label={label}
    readonly={draft.sending}
    spellcheck="true"
    onfocus={() => (focused = true)}
    onblur={() => (focused = false)}
    onkeydown={onKeydown}
    data-testid="{testid}-field"
  ></textarea>
  {#if draft.error !== null && !draft.sending}
    <p class="failed" role="alert" data-testid="{testid}-error">
      <span class="failed-text" title={draft.error}>{draft.error}</span>
      {#if draft.logFrom > 0}
        <button class="log" type="button" onclick={() => onlog(draft.logFrom)}>Show log</button>
      {/if}
    </p>
  {/if}
  {#if open || lead}
    <div class="actions">
      {#if lead}{@render lead()}{/if}
      <span class="spacer"></span>
      {#if open}
        <button
          class="send"
          type="button"
          disabled={!canSend}
          title="{sendLabel} ({SEND_KEYS})"
          onmousedown={(e) => e.preventDefault()}
          onclick={send}
          data-testid="{testid}-send"
        >
          {#if draft.sending}{@render spinner()}Sending…{:else}{sendLabel}{/if}
        </button>
      {/if}
    </div>
  {/if}
</div>

<style>
  .composer {
    display: flex;
    flex-direction: column;
    gap: 6px;
    min-width: 0;
  }

  /* One quiet line until it is used: the card's own colours, a faint edge. */
  .field {
    display: block;
    width: 100%;
    min-width: 0;
    height: 28px;
    padding: 4px 9px;
    border: 1px solid var(--border);
    border-radius: 6px;
    background: color-mix(in srgb, var(--field-bg) 55%, transparent);
    color: var(--text);
    font: inherit;
    font-size: var(--fs-sm);
    line-height: 18px;
    overflow: hidden;
    resize: none;
    transition:
      height 140ms ease-out,
      border-color 80ms linear,
      background-color 80ms linear,
      box-shadow 80ms linear;
  }

  .field::placeholder {
    color: var(--text-faint);
  }

  .field:hover {
    border-color: var(--border-strong);
  }

  .open .field {
    padding: 6px 9px;
    border-color: var(--border-strong);
    background: var(--field-bg);
    line-height: 1.45;
  }

  .field:focus {
    border-color: var(--accent);
    outline: none;
    background: var(--field-bg);
    box-shadow: 0 0 0 3px color-mix(in srgb, var(--accent) 22%, transparent);
  }

  .field.sending {
    color: var(--text-muted);
  }

  @media (prefers-reduced-motion: reduce) {
    .field {
      transition: none;
    }
  }

  .actions {
    display: flex;
    align-items: center;
    gap: 6px;
    min-width: 0;
    min-height: 24px;
  }

  .spacer {
    flex: 1 1 0;
    min-width: 0;
  }

  .send {
    display: inline-flex;
    align-items: center;
    gap: 5px;
    flex: none;
    height: 24px;
    padding: 0 10px;
    border: 1px solid var(--accent);
    border-radius: 5px;
    background: var(--accent);
    color: var(--accent-contrast);
    font-size: var(--fs-xs);
    font-weight: 600;
    white-space: nowrap;
    transition:
      background-color 80ms linear,
      border-color 80ms linear;
  }

  .send:hover:not(:disabled) {
    border-color: var(--accent-hover);
    background: var(--accent-hover);
  }

  .send:focus-visible {
    outline: 2px solid var(--focus-ring);
    outline-offset: 2px;
  }

  .send:disabled {
    border-color: var(--border-strong);
    background: var(--bg-pressed);
    color: var(--text-faint);
  }

  .spin {
    flex: none;
    animation: spin 0.8s linear infinite;
  }

  /* Why it was not sent: as the tab's own failed line. */
  .failed {
    display: flex;
    align-items: center;
    gap: 8px;
    min-width: 0;
    margin: 0;
    padding: 4px 8px 4px 9px;
    border: 1px solid color-mix(in srgb, var(--danger) 30%, var(--border));
    border-radius: 6px;
    background: var(--danger-tint);
    color: var(--text);
    font-size: var(--fs-xs);
    line-height: 1.4;
  }

  .failed-text {
    flex: 1 1 auto;
    min-width: 0;
    overflow-wrap: anywhere;
  }

  .log {
    flex: none;
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    font-size: var(--fs-xs);
    font-weight: 550;
  }

  .log:hover {
    text-decoration: underline;
    text-underline-offset: 2px;
  }
</style>
