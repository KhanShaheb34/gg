<!--
  Create a branch from a stash: a name (checked as it is typed, the core checking again).
  git makes the branch at the commit the stash was saved on, checks it out, applies the
  stash there and drops it. Stays open until the core has answered: a name it refuses shows
  under the field, the text kept.
-->
<script lang="ts">
  import { refNameProblem } from "../../nameCheck";
  import Dialog from "../Dialog.svelte";
  import CommitRef from "./CommitRef.svelte";
  import NameField from "./NameField.svelte";
  import { NamedSubmit } from "./named.svelte";

  type Props = {
    repoId: number;
    /** The stash's commit. */
    stash: string;
    subject: string;
    /** Local branches: their names are taken. */
    branches: string[];
    /** The active worktree it was opened for: what it asks for runs only there. */
    worktree: string;
    close: () => void;
  };

  let { repoId, stash, subject, branches, worktree, close }: Props = $props();

  let name = $state("");
  // svelte-ignore state_referenced_locally
  const named = new NamedSubmit(repoId, close);
  const problem = $derived(named.problem(name, refNameProblem(name, branches, "branch")));

  function submit() {
    void named.submit(name, { op: "stash_branch", stash, name, expectWorktree: worktree });
  }
</script>

<Dialog
  title="Create branch from stash"
  primary={named.pending ? "Creating…" : "Create branch"}
  valid={problem === null && !named.pending}
  once={false}
  cancellable={!named.pending}
  onSubmit={submit}
  onCancel={close}
>
  <div class="dialog-target">
    <span class="dialog-target-label">Stash</span>
    <CommitRef hash={stash} {subject} />
  </div>
  <NameField label="Branch name" bind:value={name} {problem} readonly={named.pending} waiting={named.waitingFor} />
  <p class="dialog-text muted">
    The branch starts where the stash was saved and is checked out; the stash is applied there, then dropped.
  </p>
</Dialog>
