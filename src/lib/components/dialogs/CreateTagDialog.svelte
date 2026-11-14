<!--
  Create tag here: a name and an optional message; with a message the tag is annotated. Like
  creating a branch, it stays open until the core has answered, so that a name it refuses
  (taken, say) shows under the field with the text kept.
-->
<script lang="ts">
  import { refNameProblem } from "../../nameCheck";
  import Dialog from "../Dialog.svelte";
  import CommitRef from "./CommitRef.svelte";
  import NameField from "./NameField.svelte";
  import { NamedSubmit } from "./named.svelte";

  type Props = { repoId: number; hash: string; subject: string; close: () => void };

  let { repoId, hash, subject, close }: Props = $props();

  const uid = Math.random().toString(36).slice(2, 8);
  let name = $state("");
  let message = $state("");
  // svelte-ignore state_referenced_locally
  const named = new NamedSubmit(repoId, close);
  // Tags are not all known here (only loaded rows carry them): the core says if one is taken.
  const problem = $derived(named.problem(name, refNameProblem(name, [], "tag")));
  const annotated = $derived(message.trim() !== "");

  function submit() {
    void named.submit(name, { op: "create_tag", name, hash, message: annotated ? message : null });
  }
</script>

<Dialog
  title="Create tag"
  primary={named.pending ? "Creating…" : "Create tag"}
  valid={problem === null && !named.pending}
  once={false}
  onSubmit={submit}
  onCancel={close}
>
  <div class="dialog-target">
    <span class="dialog-target-label">At</span>
    <CommitRef {hash} {subject} />
  </div>
  <NameField label="Tag name" bind:value={name} {problem} readonly={named.pending} />
  <div class="field">
    <label class="field-label" for="tag-message-{uid}">Message <span class="optional">(optional)</span></label>
    <textarea
      id="tag-message-{uid}"
      class="text-input"
      rows="3"
      bind:value={message}
      readonly={named.pending}
      aria-describedby="tag-kind-{uid}"
    ></textarea>
    <p class="field-hint" id="tag-kind-{uid}">
      {annotated
        ? "Annotated: it records the message, who made it and when."
        : "Without a message the tag is lightweight: a name for the commit, nothing more."}
    </p>
  </div>
</Dialog>
