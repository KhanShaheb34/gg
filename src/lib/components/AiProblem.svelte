<!--
  Why "Generate with AI" gave no text, under the field it was for, in the panel's error style,
  with the next step: a terminal to sign in, the settings for a model, Try again, and the
  command log for what the tool said.
-->
<script lang="ts">
  import { openExternal } from "../state/editor";
  import { showLog } from "../state/app.svelte";
  import { ai, toolInfo } from "../state/ai.svelte";
  import { openSettings } from "../state/settings.svelte";
  import type { AiOutcome } from "../types";

  type Failure = Extract<AiOutcome, { status: "failed" }>;
  let { failure, repoId, onretry, testid }: { failure: Failure; repoId: number; onretry: () => void; testid: string } = $props();

  const info = $derived(failure.tool ? toolInfo(failure.tool, ai.tools) : null);
  const retry = $derived(["timed_out", "failed", "empty", "limited"].includes(failure.kind));
</script>

<div class="problem" role="alert" data-testid={testid} data-kind={failure.kind}>
  <svg width="13" height="13" viewBox="0 0 16 16" aria-hidden="true">
    <circle cx="8" cy="8" r="6.3" fill="none" stroke="currentColor" stroke-width="1.4" />
    <path d="M8 4.6v4.2" stroke="currentColor" stroke-width="1.6" stroke-linecap="round" />
    <circle cx="8" cy="11.1" r="0.95" fill="currentColor" />
  </svg>
  <div class="body">
    {#if failure.kind === "signed_out" && info}
      <p>{info.name} is not signed in. Run <code>{info.signIn}</code> in a terminal to sign in, then try again.</p>
    {:else}
      <p>{failure.message}</p>
    {/if}
    <div class="actions">
      {#if failure.kind === "signed_out"}
        <button type="button" class="link-btn" onclick={() => void openExternal(repoId, { target: "terminal" })}>Open terminal</button>
      {/if}
      {#if failure.kind === "bad_model" || failure.kind === "no_tool"}
        <button type="button" class="link-btn" onclick={() => openSettings()}>Settings</button>
      {/if}
      {#if retry}
        <button type="button" class="link-btn" onclick={onretry}>Try again</button>
      {/if}
      {#if failure.logFrom > 0 && failure.kind !== "nothing"}
        <button type="button" class="link-btn" onclick={() => showLog(repoId, failure.logFrom)}>Show log</button>
      {/if}
    </div>
  </div>
</div>

<style>
  .problem {
    display: flex;
    align-items: flex-start;
    gap: 6px;
    margin: 0;
    color: var(--danger);
    font-size: var(--fs-xs);
    line-height: 1.45;
    overflow-wrap: anywhere;
  }

  .problem > svg {
    flex: none;
    margin-top: 1.5px;
  }

  .body {
    display: flex;
    flex-direction: column;
    gap: 3px;
    min-width: 0;
  }

  code {
    padding: 0 3px;
    border-radius: 4px;
    background: var(--danger-tint);
    font-family: var(--font-mono);
    font-size: 0.95em;
  }

  .actions {
    display: flex;
    flex-wrap: wrap;
    gap: 2px 12px;
  }

  .actions:empty {
    display: none;
  }

  .link-btn {
    padding: 0;
    border: none;
    background: none;
    color: var(--accent);
    font-size: inherit;
    font-weight: 550;
    cursor: pointer;
  }

  .link-btn:hover {
    text-decoration: underline;
  }

  .link-btn:focus-visible {
    border-radius: 3px;
    outline: 2px solid var(--focus-ring);
    outline-offset: 2px;
  }
</style>
