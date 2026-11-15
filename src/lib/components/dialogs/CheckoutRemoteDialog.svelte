<!--
  Check out a remote branch as a local branch that tracks it: the local name, prefilled with
  the branch's name on the remote. If a local branch of that name exists, the dialog offers
  to check that one out instead (as its own menu would), or another name can be typed.
-->
<script lang="ts">
  import { checkOutBranch } from "../../menus/checkout";
  import { remoteOf, shown } from "../../menus/common";
  import { refNameProblem } from "../../nameCheck";
  import Dialog from "../Dialog.svelte";
  import NameField from "./NameField.svelte";
  import { NamedSubmit } from "./named.svelte";

  type Props = {
    repoId: number;
    /** "origin/feature/x". */
    remoteBranch: string;
    remotes: string[];
    /** Local branches. */
    branches: string[];
    /** The active worktree it was opened for: what it asks for runs only there. */
    worktree: string;
    close: () => void;
  };

  let { repoId, remoteBranch, remotes, branches, worktree, close }: Props = $props();

  // svelte-ignore state_referenced_locally
  const remote = remoteOf(remoteBranch, remotes);
  // svelte-ignore state_referenced_locally
  let name = $state(remote === null ? remoteBranch : remoteBranch.slice(remote.length + 1));
  // svelte-ignore state_referenced_locally
  const named = new NamedSubmit(repoId, close);
  const exists = $derived(branches.includes(name));
  const problem = $derived(exists ? null : named.problem(name, refNameProblem(name, [], "branch")));

  function submit() {
    if (exists) {
      close();
      checkOutBranch(repoId, name, worktree);
      return;
    }
    void named.submit(name, { op: "checkout_remote", remoteBranch, localName: name, expectWorktree: worktree });
  }
</script>

<Dialog
  title="Check out {shown(remoteBranch)} as a local branch"
  primary={exists ? `Check out ${shown(name)}` : named.pending ? "Checking out…" : "Check out"}
  valid={problem === null && !named.pending}
  once={false}
  cancellable={!named.pending}
  onSubmit={submit}
  onCancel={close}
>
  <NameField
    label="Local branch name"
    bind:value={name}
    {problem}
    note={exists ? `A branch named ${name} exists here: check it out instead, or choose another name.` : null}
    readonly={named.pending}
    waiting={named.waitingFor}
  />
</Dialog>
