<!--
  Create worktree: what it checks out (an existing branch no worktree has, a new branch at a
  commit, or that commit detached when started from one) and where its folder goes,
  suggested by the location pattern in the settings for the branch chosen (`worktree_default_path`)
  and editable; and whether to make it active once made (off to begin with). Checked as it is
  filled in; the core checks again in its turn, and what it refuses (a folder that is not
  empty, say) shows here, the dialog kept open. The home folder shows as ~ and may be typed so.
-->
<script lang="ts" module>
  /** Where a dialog starts from: HEAD (the panel's add button), a local branch's label, a commit. */
  export type CreateFrom =
    | { kind: "head"; hash: string; branch: string | null }
    | { kind: "branch"; name: string; hash: string; checkedOutIn: string | null }
    | { kind: "commit"; hash: string; subject: string };
</script>

<script lang="ts">
  import { onDestroy } from "svelte";
  import { shortHash, shortenPath, visibleText } from "../../format";
  import { CoreError, api } from "../../ipc";
  import { shown } from "../../menus/common";
  import { refNameProblem } from "../../nameCheck";
  import { isRefusal, runOp } from "../../state/ops.svelte";
  import { findRepoView } from "../../state/repo.svelte";
  import { makeActive } from "../../state/worktrees.svelte";
  import type { OpRequest, WorktreeBranch, WorktreeInfo } from "../../types";
  import Dialog from "../Dialog.svelte";
  import CommitRef from "./CommitRef.svelte";
  import NameField from "./NameField.svelte";

  type Props = {
    repoId: number;
    from: CreateFrom;
    /** Local branches. */
    branches: string[];
    /** The repository's worktrees: their branches are taken, their folders too. */
    worktrees: WorktreeInfo[];
    homeDir: string | null;
    close: () => void;
  };

  let { repoId, from, branches, worktrees, homeDir, close }: Props = $props();

  type Mode = "existing" | "create" | "detach";
  const uid = Math.random().toString(36).slice(2, 8);

  /** The worktree each branch is checked out in. */
  // svelte-ignore state_referenced_locally
  const takenBy = new Map(worktrees.filter((w) => w.branch !== null).map((w) => [w.branch!, w.path]));
  // svelte-ignore state_referenced_locally
  const free = branches.filter((b) => !takenBy.has(b));
  // svelte-ignore state_referenced_locally
  const start = from;
  /** The branch the dialog was started from, when a worktree can check it out. */
  const fromBranch = start.kind === "branch" && start.checkedOutIn === null ? start.name : null;

  let mode: Mode = $state(fromBranch !== null ? "existing" : "create");
  let existing = $state(fromBranch ?? free[0] ?? "");
  let name = $state("");
  let location = $state("");
  /** The user typed in the location: suggestions no longer replace it. */
  let edited = $state(false);
  /** The suggested folder is there already, empty. */
  let suggestedExists = $state(false);
  let makeActiveAfter = $state(false);
  let pending = $state(false);
  /** What the core said of the request last sent, and of which field. */
  let refused: { field: "name" | "location"; message: string; sent: string } | null = $state(null);
  let open = true;
  onDestroy(() => (open = false));

  /** What a new branch or a detached HEAD starts at, as said in the dialog. */
  const atLabel = start.kind === "head" ? (start.branch ?? shortHash(start.hash)) : start.kind === "branch" ? start.name : null;

  /** `text` with a leading `~` as the home folder. */
  function expand(text: string): string {
    const t = text.trim();
    if (homeDir && (t === "~" || t.startsWith("~/") || t.startsWith("~\\"))) return homeDir + t.slice(1);
    return t;
  }

  const sep = (p: string) => (p.includes("\\") && !p.includes("/") ? "\\" : "/");

  /** The listed worktree `path` is, or is inside, if any. */
  function worktreeAt(path: string): { at: WorktreeInfo; inside: boolean } | null {
    const p = path.replace(/[\\/]+$/, "");
    for (const w of worktrees) {
      if (p === w.path) return { at: w, inside: false };
      if (p.startsWith(w.path + sep(w.path))) return { at: w, inside: true };
    }
    return null;
  }

  /** The branch name the location is suggested for. */
  const suggestFor = $derived(
    mode === "existing" ? existing : mode === "create" ? (refNameProblem(name, branches) === null ? name : "") : shortHash(start.hash),
  );

  // Suggest a folder for the branch chosen, a moment after it stops changing.
  let asked = 0;
  $effect(() => {
    const branch = suggestFor;
    if (branch === "" || edited) return;
    const request = ++asked;
    const timer = setTimeout(async () => {
      try {
        const suggestion = await api.worktreeDefaultPath(repoId, branch);
        if (request !== asked || edited || !open) return;
        location = shortenPath(suggestion.path, homeDir);
        suggestedExists = suggestion.exists;
      } catch {
        // The user types one.
      }
    }, 120);
    return () => clearTimeout(timer);
  });

  const nameProblem = $derived.by(() => {
    if (mode !== "create") return null;
    if (refused?.field === "name" && refused.sent === name) return refused.message;
    return refNameProblem(name, branches, "branch");
  });

  const locationProblem = $derived.by(() => {
    if (refused?.field === "location" && refused.sent === location) return refused.message;
    const path = expand(location);
    if (path === "") return "Choose where the worktree’s folder goes.";
    const there = worktreeAt(path);
    if (there && !there.inside) return "There is a worktree there already.";
    if (there) return `That is inside the worktree at ${shortenPath(there.at.path, homeDir)}: a worktree cannot go inside another.`;
    return null;
  });

  const locationNote = $derived(
    !edited && suggestedExists ? "An empty folder is there already; the worktree goes into it." : "A new folder, or an empty one.",
  );

  const valid = $derived(
    !pending && locationProblem === null && (mode === "existing" ? existing !== "" : mode === "create" ? nameProblem === null : true),
  );

  function branchOf(): WorktreeBranch {
    if (mode === "existing") return { existing };
    if (mode === "create") return { create: name, at: start.hash };
    return { detach: start.hash };
  }

  async function submit() {
    if (!valid) return;
    const request: OpRequest = { op: "worktree_add", path: expand(location), branch: branchOf() };
    const sent = { name, location };
    pending = true;
    try {
      const outcome = await runOp(repoId, request, { ownErrors: ["bad_name", "bad_request"], ownRefusals: true });
      if (isRefusal(outcome) && open) {
        const field = /branch named/.test(outcome.message) ? "name" : "location";
        refused = { field, message: outcome.message, sent: field === "name" ? sent.name : sent.location };
        pending = false;
        return;
      }
      close();
      // Exactly the worktree this request made, never one another program added meanwhile.
      if (outcome.status === "done" && makeActiveAfter && outcome.created !== null) {
        void makeActive(repoId, outcome.created, findRepoView(repoId));
      }
    } catch (e) {
      if (e instanceof CoreError && (e.code === "bad_name" || e.code === "bad_request") && open) {
        const field = e.code === "bad_name" ? "name" : "location";
        refused = { field, message: e.message, sent: field === "name" ? sent.name : sent.location };
        pending = false;
        return;
      }
      close(); // reported by runOp
    }
  }
</script>

<Dialog
  title="Create worktree"
  primary={pending ? "Creating…" : "Create worktree"}
  {valid}
  once={false}
  cancellable={!pending}
  onSubmit={() => void submit()}
  onCancel={close}
>
  {#if start.kind === "branch" && start.checkedOutIn !== null}
    <p class="dialog-text muted lead" data-testid="taken-note">
      {shown(start.name)} is checked out in {shortenPath(start.checkedOutIn, homeDir)}. A worktree needs a branch of its own:
      start a new one at it.
    </p>
  {/if}
  <fieldset class="choices">
    <legend class="field-label">Check out</legend>
    <label class="choice" class:off={free.length === 0}>
      <input class="radio" type="radio" name="wt-mode-{uid}" value="existing" bind:group={mode} disabled={free.length === 0 || pending} />
      <span>
        <span class="choice-title">An existing branch</span>
        <span class="choice-text">
          {free.length === 0 ? "Every local branch is checked out in a worktree already." : "One no worktree has checked out."}
        </span>
      </span>
    </label>
    <label class="choice">
      <input class="radio" type="radio" name="wt-mode-{uid}" value="create" bind:group={mode} disabled={pending} />
      <span>
        <span class="choice-title">A new branch</span>
        <span class="choice-text">
          {#if atLabel !== null}Starting at <span class="ref">{shown(atLabel)}</span>.{:else}Starting at this commit.{/if}
        </span>
      </span>
    </label>
    {#if start.kind === "commit"}
      <label class="choice">
        <input class="radio" type="radio" name="wt-mode-{uid}" value="detach" bind:group={mode} disabled={pending} />
        <span>
          <span class="choice-title">This commit, detached</span>
          <span class="choice-text">On no branch: for a look around or a quick build.</span>
        </span>
      </label>
    {/if}
  </fieldset>

  {#if mode === "existing"}
    <div class="field">
      <label class="field-label" for="wt-branch-{uid}">Branch</label>
      <div class="select-wrap">
        <select id="wt-branch-{uid}" class="select" bind:value={existing} disabled={pending}>
          {#if existing === ""}
            <option value="" disabled>Choose a branch…</option>
          {/if}
          {#each branches as b (b)}
            {@const taken = takenBy.get(b)}
            <option value={b} disabled={taken !== undefined}
              >{shown(b)}{taken === undefined ? "" : ` (in ${shortenPath(taken, homeDir)})`}</option
            >
          {/each}
        </select>
      </div>
    </div>
  {:else if mode === "create"}
    <NameField label="New branch name" bind:value={name} problem={nameProblem} readonly={pending} />
    {#if start.kind === "commit"}
      <div class="dialog-target at">
        <span class="dialog-target-label">At</span>
        <CommitRef hash={start.hash} subject={start.subject} />
      </div>
    {/if}
  {:else}
    <div class="dialog-target at">
      <span class="dialog-target-label">At</span>
      <CommitRef hash={start.hash} subject={start.kind === "commit" ? start.subject : null} />
    </div>
  {/if}

  <div class="field">
    <label class="field-label" for="wt-location-{uid}">Location</label>
    <input
      id="wt-location-{uid}"
      class="text-input path"
      type="text"
      bind:value={location}
      oninput={() => (edited = true)}
      readonly={pending}
      placeholder={mode === "create" && name === "" ? "Suggested once the branch has a name" : ""}
      autocomplete="off"
      autocapitalize="off"
      spellcheck="false"
      aria-invalid={locationProblem !== null && location !== "" && (edited || refused !== null)}
      aria-describedby="wt-location-hint-{uid}"
      data-testid="worktree-location"
    />
    <p
      class="field-hint"
      class:problem={locationProblem !== null && location !== "" && (edited || refused !== null)}
      id="wt-location-hint-{uid}"
      aria-live="polite"
    >
      {visibleText(locationProblem !== null && location !== "" ? locationProblem : locationNote)}
    </p>
  </div>

  <label class="check-row">
    <input class="checkbox" type="checkbox" bind:checked={makeActiveAfter} disabled={pending} />
    <span>
      Make it the active worktree once created
      <span class="check-note">Actions then run there; the worktree you are in now stays as it is.</span>
    </span>
  </label>
</Dialog>

<style>
  .lead {
    margin-bottom: 12px;
  }

  .choices {
    margin-top: 4px;
  }

  .choice.off {
    opacity: 0.6;
  }

  .ref {
    color: var(--text);
    font-weight: 600;
  }

  .at {
    margin-top: 2px;
  }

  .path {
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
  }
</style>
