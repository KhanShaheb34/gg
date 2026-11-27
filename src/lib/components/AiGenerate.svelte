<!--
  "Generate with AI" at a field (the commit message, the pull request's title): a small quiet
  button with a sparkle, which turns into a spinner, the same size, while the tool writes
  (clicking it then cancels). It says in its tooltip which tool and model will run.

  Before a tool runs the first time, a small popover says what is sent, to whom (Continue /
  Cancel, remembered per tool). With no tool installed (or the one chosen in the settings
  missing) the button stays, muted, and its popover says so, with the official install pages
  and Check again. Nothing to describe: the button is unavailable, and says why in its tooltip.

  The parent runs the generation (`onrun`) and cancels it (`oncancel`); `running` is its own.
-->
<script lang="ts">
  import { onMount, tick } from "svelte";
  import { api } from "../ipc";
  import { isMac } from "../platform";
  import { app } from "../state/app.svelte";
  import { ai, chosenTool, ensureAi, missingChoice, noticeSeen, noticeText, refreshAi, rememberNotice, runsWith } from "../state/ai.svelte";
  import { openSettings } from "../state/settings.svelte";
  import type { AiTask } from "../types";

  type Props = {
    kind: AiTask["kind"];
    /** Why there is nothing to generate from now (said in the tooltip), or null. */
    blocked: string | null;
    running: boolean;
    onrun: () => void;
    oncancel: () => void;
    testid: string;
  };
  let { kind, blocked, running, onrun, oncancel, testid }: Props = $props();

  const KEYS = isMac() ? "⌘G" : "Ctrl+G";
  const uid = Math.random().toString(36).slice(2, 8);

  onMount(ensureAi);

  const tool = $derived(chosenTool(ai.tools, app.settings));
  const missing = $derived(missingChoice(ai.tools, app.settings));
  /** Looked, and nothing runs. */
  const none = $derived(ai.tools !== null && tool === null);

  let open = $state<null | "notice" | "missing">(null);
  let button: HTMLButtonElement | undefined = $state();
  let popover: HTMLDivElement | undefined = $state();

  const what = $derived(kind === "commit" ? "a commit message" : "the title and description");

  const title = $derived.by(() => {
    if (running) return "Cancel (Esc)";
    if (ai.tools === null) return `Generate ${what} with AI`;
    if (tool === null)
      return missing ? `Generate with AI: ${missing.name} was not found` : "Generate with AI: no AI tool was found on this computer";
    if (blocked !== null) return blocked;
    return `Generate with ${runsWith(tool, app.settings)} (${KEYS})`;
  });

  /** The click, or the shortcut in the field. */
  export async function activate(): Promise<void> {
    if (running) {
      oncancel();
      return;
    }
    if (ai.tools === null) await refreshAi();
    if (chosenTool(ai.tools, app.settings) === null) {
      await show("missing");
      return;
    }
    if (blocked !== null) return;
    const t = chosenTool(ai.tools, app.settings)!;
    if (!noticeSeen(t.tool, app.settings)) {
      await show("notice");
      return;
    }
    onrun();
  }

  /** Where the popover is drawn (fixed, so no scrolling panel clips it). */
  let place = $state({ top: 0, left: 0, above: false });

  /** Under the button, its right edge on the button's; above it when there is no room below. */
  function position() {
    if (!button || !popover) return;
    const b = button.getBoundingClientRect();
    const w = popover.offsetWidth;
    const h = popover.offsetHeight;
    const left = Math.max(8, Math.min(b.right + 4 - w, window.innerWidth - w - 8));
    const below = b.bottom + 6;
    const above = below + h > window.innerHeight - 8 && b.top - 6 - h >= 8;
    place = { top: above ? b.top - 6 - h : below, left, above };
  }

  async function show(which: "notice" | "missing") {
    open = which;
    await tick();
    position();
    popover?.querySelector<HTMLElement>("[data-autofocus]")?.focus();
  }

  function close(refocus = true) {
    open = null;
    if (refocus) button?.focus();
  }

  async function goOn() {
    const t = tool;
    if (t === null) return;
    close();
    await rememberNotice(t.tool);
    if (blocked === null) onrun();
  }

  async function checkAgain() {
    await refreshAi({ recheck: true });
    const t = chosenTool(ai.tools, app.settings);
    if (t === null) return;
    // Found now: the notice, if it was never seen, else ready to click.
    if (!noticeSeen(t.tool, app.settings)) await show("notice");
    else close();
  }

  async function openLink(link: "installClaude" | "installCodex") {
    try {
      await api.openAppLink(link);
    } catch {
      // The page is named beside the link too.
    }
  }

  function onWindowPointer(e: PointerEvent) {
    if (open === null) return;
    const target = e.target as Node;
    if (popover?.contains(target) || button?.contains(target)) return;
    close(false);
  }

  function onPopoverKeydown(e: KeyboardEvent) {
    if (e.key === "Escape") {
      e.preventDefault();
      e.stopPropagation();
      close();
    }
  }

  // The choice may change while it is open (settings, another window): it follows.
  $effect(() => {
    if (open === "missing" && tool !== null && noticeSeen(tool.tool, app.settings)) open = null;
  });
</script>

<svelte:window onpointerdown={onWindowPointer} onresize={() => open !== null && position()} />

<span class="ai">
  <button
    bind:this={button}
    type="button"
    class="ai-btn"
    class:muted={none}
    class:running
    aria-disabled={!running && !none && blocked !== null ? "true" : undefined}
    aria-label={running ? "Cancel generating" : `Generate ${what} with AI`}
    aria-haspopup="dialog"
    aria-expanded={open !== null}
    aria-controls={open !== null ? `ai-pop-${uid}` : undefined}
    {title}
    data-testid={testid}
    data-state={running ? "running" : none ? "none" : blocked !== null ? "blocked" : "ready"}
    onclick={() => void activate()}
  >
    {#if running}
      <svg class="spin" width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
        <circle cx="8" cy="8" r="5.6" fill="none" stroke="currentColor" stroke-opacity="0.25" stroke-width="1.8" />
        <path d="M8 2.4a5.6 5.6 0 0 1 5.6 5.6" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" />
      </svg>
    {:else}
      <!-- A sparkle: a four-pointed star and a small one. -->
      <svg width="14" height="14" viewBox="0 0 16 16" aria-hidden="true">
        <path
          d="M6.5 2.2c.35 2.6 1.55 3.85 4.1 4.2-2.55.35-3.75 1.6-4.1 4.2-.35-2.6-1.55-3.85-4.1-4.2 2.55-.35 3.75-1.6 4.1-4.2Z"
          fill="none"
          stroke="currentColor"
          stroke-width="1.3"
          stroke-linejoin="round"
        />
        <path d="M12 9.6c.18 1.25.75 1.85 2 2.02-1.25.18-1.82.78-2 2.03-.18-1.25-.75-1.85-2-2.03 1.25-.17 1.82-.77 2-2.02Z" fill="currentColor" />
      </svg>
    {/if}
  </button>

  {#if open !== null}
    <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
    <div
      bind:this={popover}
      id="ai-pop-{uid}"
      class="pop"
      class:above={place.above}
      style:top="{place.top}px"
      style:left="{place.left}px"
      role="dialog"
      aria-labelledby="ai-pop-title-{uid}"
      tabindex="-1"
      onkeydown={onPopoverKeydown}
      data-testid="{testid}-popover"
    >
      {#if open === "notice" && tool !== null}
        <p class="pop-title" id="ai-pop-title-{uid}">Generate with {tool.name}</p>
        <p class="pop-text" data-testid="ai-notice-text">{noticeText(tool, kind)}</p>
        <p class="pop-fine">
          {tool.tool === "codex" ? "Codex runs with its commands off, read-only" : `${tool.name} runs with no tools`}, outside the repository. Asked
          once; the tool and its prompts are in Settings.
        </p>
        <div class="pop-buttons">
          <button type="button" class="pbtn" onclick={() => close()}>Cancel</button>
          <button type="button" class="pbtn primary" data-autofocus data-testid="ai-notice-continue" onclick={() => void goOn()}>Continue</button>
        </div>
      {:else}
        <p class="pop-title" id="ai-pop-title-{uid}">{missing ? `${missing.name} was not found` : "No AI tool found"}</p>
        {#if missing}
          <p class="pop-text" data-testid="ai-missing-text">
            Settings choose {missing.name}, which was not found on this computer. Install it and sign in, or choose Automatic.
          </p>
          <div class="pop-links">
            <button type="button" class="plink" onclick={() => void openLink(missing.tool === "claude" ? "installClaude" : "installCodex")}
              >Get {missing.name}<span aria-hidden="true"> ↗</span></button
            >
          </div>
        {:else}
          <p class="pop-text" data-testid="ai-missing-text">
            Generate with AI uses Claude Code or Codex installed on this computer, signed in to your own account. Neither was found.
          </p>
          <div class="pop-links">
            <button type="button" class="plink" data-testid="ai-get-claude" onclick={() => void openLink("installClaude")}
              >Get Claude Code<span aria-hidden="true"> ↗</span></button
            >
            <button type="button" class="plink" data-testid="ai-get-codex" onclick={() => void openLink("installCodex")}
              >Get Codex<span aria-hidden="true"> ↗</span></button
            >
          </div>
        {/if}
        <div class="pop-buttons">
          {#if missing}
            <button
              type="button"
              class="pbtn"
              onclick={() => {
                close(false);
                openSettings();
              }}>Settings</button
            >
          {/if}
          <button
            type="button"
            class="pbtn primary"
            data-autofocus
            disabled={ai.checking}
            data-testid="ai-check-again"
            onclick={() => void checkAgain()}>{ai.checking ? "Checking…" : "Check again"}</button
          >
        </div>
      {/if}
    </div>
  {/if}
</span>

<style>
  .ai {
    position: relative;
    display: inline-flex;
    flex: none;
  }

  .ai-btn {
    display: inline-grid;
    place-items: center;
    width: 22px;
    height: 22px;
    padding: 0;
    border: 0;
    border-radius: 5px;
    background: transparent;
    color: var(--text-muted);
    transition:
      background-color 80ms linear,
      color 80ms linear;
  }

  .ai-btn:hover {
    background: var(--bg-hover);
    color: var(--accent);
  }

  .ai-btn:active {
    background: var(--bg-pressed);
  }

  .ai-btn:focus-visible {
    outline: 2px solid var(--focus-ring);
    outline-offset: 1px;
  }

  .ai-btn.running {
    color: var(--accent);
  }

  /* No tool, or nothing to describe: quiet, still there (its tooltip or popover says why). */
  .ai-btn.muted,
  .ai-btn[aria-disabled="true"] {
    color: var(--text-faint);
    opacity: 0.7;
  }

  .ai-btn[aria-disabled="true"]:hover {
    background: transparent;
    color: var(--text-faint);
  }

  .ai-btn.muted:hover {
    color: var(--text-muted);
    opacity: 1;
  }

  .spin {
    animation: spin 0.8s linear infinite;
  }

  @media (prefers-reduced-motion: reduce) {
    .spin {
      animation-duration: 2.4s;
    }
  }

  /* Fixed over the window (see `position`): no scrolling panel clips it. */
  .pop {
    position: fixed;
    z-index: 60;
    display: flex;
    flex-direction: column;
    gap: 8px;
    width: 252px;
    max-width: calc(100vw - 24px);
    padding: 12px 12px 10px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius-lg);
    background: var(--bg-elevated);
    box-shadow: var(--shadow-pop);
    color: var(--text);
    font-size: var(--fs-sm);
    line-height: 1.45;
    text-align: left;
    white-space: normal;
    cursor: default;
    animation: pop-in 120ms var(--ease);
  }

  .pop.above {
    animation-name: pop-in-above;
  }

  @keyframes pop-in {
    from {
      opacity: 0;
      transform: translateY(-3px);
    }
  }

  @keyframes pop-in-above {
    from {
      opacity: 0;
      transform: translateY(3px);
    }
  }

  @media (prefers-reduced-motion: reduce) {
    .pop {
      animation: none;
    }
  }

  .pop-title {
    font-size: var(--fs);
    font-weight: 650;
  }

  .pop-text {
    color: var(--text);
  }

  .pop-fine {
    color: var(--text-muted);
    font-size: var(--fs-xs);
  }

  .pop-links {
    display: flex;
    flex-wrap: wrap;
    gap: 4px 14px;
  }

  .plink {
    padding: 0;
    border: 0;
    background: none;
    color: var(--accent);
    font-size: var(--fs-sm);
    font-weight: 550;
  }

  .plink:hover {
    text-decoration: underline;
  }

  .plink:focus-visible {
    border-radius: 3px;
    outline: 2px solid var(--focus-ring);
    outline-offset: 2px;
  }

  .pop-buttons {
    display: flex;
    justify-content: flex-end;
    gap: 6px;
    margin-top: 2px;
  }

  .pbtn {
    height: 26px;
    padding: 0 11px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg);
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 550;
  }

  .pbtn:hover:not(:disabled) {
    background: var(--bg-hover);
  }

  .pbtn:focus-visible {
    outline: 2px solid var(--focus-ring);
    outline-offset: 2px;
  }

  .pbtn.primary {
    border-color: var(--accent);
    background: var(--accent);
    color: var(--accent-contrast);
  }

  .pbtn.primary:hover:not(:disabled) {
    border-color: var(--accent-hover);
    background: var(--accent-hover);
  }

  .pbtn:disabled {
    opacity: 0.6;
  }
</style>
