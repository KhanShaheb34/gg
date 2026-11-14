<!--
  Create branch here: a name (checked as it is typed, the core checking again) and whether
  to check it out (on to begin with). Stays open until the core has answered: a name it
  refuses shows under the field, the text kept.
-->
<script lang="ts">
  import { onDestroy } from "svelte";
  import { visibleText } from "../../format";
  import { refNameProblem } from "../../nameCheck";
  import Dialog from "../Dialog.svelte";
  import CommitRef from "./CommitRef.svelte";
  import { runNamed } from "./run";

  type Props = {
    repoId: number;
    hash: string;
    subject: string;
    /** Local branches: their names are taken. */
    branches: string[];
    close: () => void;
  };

  let { repoId, hash, subject, branches, close }: Props = $props();

  const uid = Math.random().toString(36).slice(2, 8);
  let name = $state("");
  let checkout = $state(true);
  let pending = $state(false);
  /** What the core said of the name last sent, while that name is still the one typed. */
  let refused = $state<{ name: string; message: string } | null>(null);
  let open = true;
  onDestroy(() => (open = false));

  const problem = $derived(
    refused !== null && refused.name === name ? refused.message : refNameProblem(name, branches, "branch"),
  );

  async function submit() {
    pending = true;
    const sent = name;
    const message = await runNamed(close, () => open, repoId, { op: "create_branch", name: sent, at: hash, checkout });
    pending = false;
    if (message !== null) refused = { name: sent, message };
  }
</script>

<Dialog
  title="Create branch"
  primary={pending ? "Creating…" : "Create branch"}
  valid={problem === null && !pending}
  once={false}
  onSubmit={submit}
  onCancel={close}
>
  <div class="dialog-target">
    <span class="dialog-target-label">At</span>
    <CommitRef {hash} {subject} />
  </div>
  <div class="field">
    <label class="field-label" for="branch-name-{uid}">Branch name</label>
    <input
      id="branch-name-{uid}"
      class="text-input"
      type="text"
      bind:value={name}
      readonly={pending}
      autocomplete="off"
      autocapitalize="off"
      spellcheck="false"
      aria-invalid={problem !== null && name !== ""}
      aria-describedby="branch-hint-{uid}"
    />
    <p class="field-hint" class:problem={problem !== null && name !== ""} id="branch-hint-{uid}" aria-live="polite">
      {problem === null ? "" : visibleText(problem)}
    </p>
  </div>
  <label class="check-row">
    <input class="checkbox" type="checkbox" bind:checked={checkout} disabled={pending} />
    <span>Check out after creating</span>
  </label>
</Dialog>
