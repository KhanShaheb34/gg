<!--
  Rename a local branch: its name, selected to be typed over, checked as it is typed (taken
  names too); the core checks again, and what it refuses shows under the field.
-->
<script lang="ts">
  import { shown } from "../../menus/common";
  import { refNameProblem } from "../../nameCheck";
  import Dialog from "../Dialog.svelte";
  import NameField from "./NameField.svelte";
  import { NamedSubmit } from "./named.svelte";

  type Props = {
    repoId: number;
    branch: string;
    /** Local branches: their names are taken. */
    branches: string[];
    close: () => void;
  };

  let { repoId, branch, branches, close }: Props = $props();

  // svelte-ignore state_referenced_locally
  let name = $state(branch);
  // svelte-ignore state_referenced_locally
  const named = new NamedSubmit(repoId, close);
  const unchanged = $derived(name === branch);
  const problem = $derived(unchanged ? null : named.problem(name, refNameProblem(name, branches, "branch")));

  function submit() {
    void named.submit(name, { op: "rename_branch", branch, newName: name });
  }
</script>

<Dialog
  title="Rename branch {shown(branch)}"
  primary={named.pending ? "Renaming…" : "Rename"}
  valid={problem === null && !unchanged && !named.pending}
  once={false}
  cancellable={!named.pending}
  onSubmit={submit}
  onCancel={close}
>
  <NameField
    label="New name"
    bind:value={name}
    {problem}
    note={unchanged ? "Type its new name." : null}
    readonly={named.pending}
    waiting={named.waitingFor}
    select
  />
</Dialog>
