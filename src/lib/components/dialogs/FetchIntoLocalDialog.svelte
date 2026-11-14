<!--
  Fetch a remote branch into a local branch without checking it out: the local name,
  prefilled with the branch's name on the remote. A new name makes a branch there; an
  existing one is fast-forwarded, if it can be (the core says when not).
-->
<script lang="ts">
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
    close: () => void;
  };

  let { repoId, remoteBranch, remotes, branches, close }: Props = $props();

  // svelte-ignore state_referenced_locally
  const remote = remoteOf(remoteBranch, remotes);
  // svelte-ignore state_referenced_locally
  let name = $state(remote === null ? remoteBranch : remoteBranch.slice(remote.length + 1));
  // svelte-ignore state_referenced_locally
  const named = new NamedSubmit(repoId, close);
  const problem = $derived(named.problem(name, refNameProblem(name, [], "branch")));
  const note = $derived(
    branches.includes(name)
      ? `${name} exists here: it is fast-forwarded to ${remoteBranch}, if it can be.`
      : `A new branch ${name} is made at ${remoteBranch}.`,
  );

  function submit() {
    void named.submit(name, { op: "fetch_into_local", remoteBranch, localName: name });
  }
</script>

<Dialog
  title="Fetch {shown(remoteBranch)} into a local branch"
  primary={named.pending ? "Fetching…" : "Fetch"}
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
    {note}
    readonly={named.pending}
    waiting={named.waitingFor}
  />
</Dialog>
