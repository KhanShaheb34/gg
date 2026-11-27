<!--
  Clone and open, from the Pull requests page, for a pull request whose repository is not open
  in gg: the folder it goes into (the one used last, else the folder of the most recent
  repository, else the home folder; typed or chosen), cloned with git into a new folder named
  after the repository there, its progress shown as it comes; Stop ends git (it removes what it
  made). Once cloned, the repository opens in a tab of its own.
-->
<script lang="ts">
  import { onDestroy, onMount } from "svelte";
  import { shortenPath } from "../../format";
  import { api, onEvent } from "../../ipc";
  import { app, openPath, reportError } from "../../state/app.svelte";
  import { closeInbox } from "../../state/inbox.svelte";
  import type { CloneProgress } from "../../types";
  import Dialog from "../Dialog.svelte";

  type Props = {
    host: string;
    owner: string;
    name: string;
    close: () => void;
  };

  let { host, owner, name, close }: Props = $props();

  /** The folder of `path` (`/a/b` → `/a`). */
  function parentOf(path: string): string {
    const cut = Math.max(path.lastIndexOf("/"), path.lastIndexOf("\\"));
    return cut > 0 ? path.slice(0, cut) : path;
  }

  let parent = $state(app.recents[0] ? parentOf(app.recents[0].path) : (app.homeDir ?? ""));
  /** The user typed or chose a folder: the one remembered, coming late, does not replace it. */
  let touched = false;
  let running = $state(false);
  let line = $state("");
  let failed = $state<{ message: string; detail: string | null } | null>(null);
  const cloneId = `clone-${Date.now().toString(36)}-${Math.random().toString(36).slice(2, 8)}`;
  const sep = $derived(parent.includes("\\") && !parent.includes("/") ? "\\" : "/");
  const into = $derived(parent.trim() === "" ? null : `${parent.trim().replace(/[\\/]+$/, "")}${sep}${name}`);

  onMount(() => {
    // The folder the last clone went into, when there was one.
    void api
      .cloneParent()
      .then((p) => {
        // Only while the field is as it opened: a folder typed or chosen meanwhile stays.
        if (p && !running && !touched) parent = p;
      })
      .catch(() => {});
  });

  const stopProgress = onEvent<CloneProgress>("clone-progress", (p) => {
    if (p.cloneId === cloneId) line = p.line;
  });
  onDestroy(stopProgress);

  async function choose() {
    let picked: string | null;
    try {
      picked = await api.pickFolder();
    } catch (e) {
      reportError(e);
      return;
    }
    if (picked) {
      touched = true;
      parent = picked;
    }
  }

  async function submit() {
    if (running || into === null) return;
    running = true;
    failed = null;
    line = "Starting…";
    let cloned;
    try {
      cloned = await api.cloneRepo(cloneId, host, owner, name, parent.trim());
    } catch (e) {
      running = false;
      failed = { message: e instanceof Error ? e.message : String(e), detail: null };
      return;
    }
    running = false;
    if (cloned.status === "done" && cloned.path) {
      close();
      closeInbox();
      await openPath(cloned.path);
      return;
    }
    if (cloned.status === "stopped") {
      line = "";
      return;
    }
    failed = { message: cloned.message, detail: cloned.detail };
  }

  function cancel() {
    if (running) {
      void api.cloneCancel(cloneId).catch(() => {});
      return;
    }
    close();
  }
</script>

<Dialog
  title="Clone {owner}/{name}"
  primary={running ? "Cloning…" : "Clone and open"}
  valid={!running && into !== null}
  once={false}
  cancelLabel={running ? "Stop" : "Cancel"}
  onSubmit={() => void submit()}
  onCancel={cancel}
>
  <div class="field">
    <label class="field-label" for="clone-parent">Into the folder</label>
    <div class="pick">
      <input
        id="clone-parent"
        class="text-input"
        type="text"
        bind:value={parent}
        oninput={() => (touched = true)}
        readonly={running}
        autocomplete="off"
        autocapitalize="off"
        spellcheck="false"
        data-testid="clone-parent"
      />
      <button class="choose" type="button" disabled={running} onclick={() => void choose()}>Choose…</button>
    </div>
    <p class="field-hint" class:problem={failed !== null} aria-live="polite" data-testid="clone-hint">
      {#if failed}
        {failed.message}
      {:else if running}
        <span class="progress" data-testid="clone-progress">{line}</span>
      {:else if into}
        A new folder, {shortenPath(into, app.homeDir)}, from {host}. It then opens in a tab.
      {:else}
        Choose the folder to clone into.
      {/if}
    </p>
    {#if failed?.detail}
      <details class="detail">
        <summary>git’s output</summary>
        <pre>{failed.detail}</pre>
      </details>
    {/if}
  </div>
</Dialog>

<style>
  .pick {
    display: flex;
    gap: 6px;
  }

  .pick .text-input {
    flex: 1 1 auto;
    min-width: 0;
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
  }

  .choose {
    flex: none;
    height: 30px;
    padding: 0 12px;
    border: 1px solid var(--border-strong);
    border-radius: var(--radius);
    background: var(--bg);
    color: var(--text);
    font-size: var(--fs-sm);
    font-weight: 550;
  }

  .choose:hover:not(:disabled) {
    background: var(--bg-hover);
  }

  .choose:disabled {
    color: var(--text-faint);
  }

  .progress {
    font-family: var(--font-mono);
    font-variant-numeric: tabular-nums;
    overflow-wrap: anywhere;
  }

  .detail {
    margin-top: 6px;
    color: var(--text-muted);
    font-size: var(--fs-xs);
  }

  .detail pre {
    max-height: 140px;
    margin: 6px 0 0;
    overflow: auto;
    font-family: var(--font-mono);
    white-space: pre-wrap;
    overflow-wrap: anywhere;
  }
</style>
