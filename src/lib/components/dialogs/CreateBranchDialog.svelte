<!--
  Create branch here: a name (checked as it is typed, the core checking again) and whether
  to check it out (on to begin with). Stays open until the core has answered: a name it
  refuses shows under the field, the text kept.
-->
<script lang="ts">
  import { refNameProblem } from "../../nameCheck";
  import Dialog from "../Dialog.svelte";
  import CommitRef from "./CommitRef.svelte";
  import NameField from "./NameField.svelte";
  import { NamedSubmit } from "./named.svelte";

  type Props = {
    repoId: number;
    hash: string;
    subject: string;
    /** Local branches: their names are taken. */
    branches: string[];
    close: () => void;
  };

  let { repoId, hash, subject, branches, close }: Props = $props();

  let name = $state("");
  let checkout = $state(true);
  // svelte-ignore state_referenced_locally
  const named = new NamedSubmit(repoId, close);
  const problem = $derived(named.problem(name, refNameProblem(name, branches, "branch")));

  function submit() {
    void named.submit(name, { op: "create_branch", name, at: hash, checkout });
  }
</script>

<Dialog
  title="Create branch"
  primary={named.pending ? "Creating…" : "Create branch"}
  valid={problem === null && !named.pending}
  once={false}
  onSubmit={submit}
  onCancel={close}
>
  <div class="dialog-target">
    <span class="dialog-target-label">At</span>
    <CommitRef {hash} {subject} />
  </div>
  <NameField label="Branch name" bind:value={name} {problem} readonly={named.pending} />
  <label class="check-row">
    <input class="checkbox" type="checkbox" bind:checked={checkout} disabled={named.pending} />
    <span>Check out after creating</span>
  </label>
</Dialog>
