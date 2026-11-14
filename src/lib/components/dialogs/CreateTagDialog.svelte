<!--
  Create tag here: a name and an optional message; with a message the tag is annotated. Like
  creating a branch, it stays open until the core has answered, so that a name it refuses
  (taken, say) shows under the field with the text kept.
-->
<script lang="ts">
  import { onDestroy } from "svelte";
  import { visibleText } from "../../format";
  import { refNameProblem } from "../../nameCheck";
  import Dialog from "../Dialog.svelte";
  import CommitRef from "./CommitRef.svelte";
  import { runNamed } from "./run";

  type Props = { repoId: number; hash: string; subject: string; close: () => void };

  let { repoId, hash, subject, close }: Props = $props();

  const uid = Math.random().toString(36).slice(2, 8);
  let name = $state("");
  let message = $state("");
  let pending = $state(false);
  let refused = $state<{ name: string; message: string } | null>(null);
  let open = true;
  onDestroy(() => (open = false));

  // Tags are not all known here (only loaded rows carry them): the core says if one is taken.
  const problem = $derived(
    refused !== null && refused.name === name ? refused.message : refNameProblem(name, [], "tag"),
  );
  const annotated = $derived(message.trim() !== "");

  async function submit() {
    pending = true;
    const sent = name;
    const request = { op: "create_tag", name: sent, hash, message: annotated ? message : null } as const;
    const said = await runNamed(close, () => open, repoId, request);
    pending = false;
    if (said !== null) refused = { name: sent, message: said };
  }
</script>

<Dialog
  title="Create tag"
  primary={pending ? "Creating…" : "Create tag"}
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
    <label class="field-label" for="tag-name-{uid}">Tag name</label>
    <input
      id="tag-name-{uid}"
      class="text-input"
      type="text"
      bind:value={name}
      readonly={pending}
      autocomplete="off"
      autocapitalize="off"
      spellcheck="false"
      aria-invalid={problem !== null && name !== ""}
      aria-describedby="tag-hint-{uid}"
    />
    <p class="field-hint" class:problem={problem !== null && name !== ""} id="tag-hint-{uid}" aria-live="polite">
      {problem === null ? "" : visibleText(problem)}
    </p>
  </div>
  <div class="field">
    <label class="field-label" for="tag-message-{uid}">Message <span class="optional">(optional)</span></label>
    <textarea
      id="tag-message-{uid}"
      class="text-input"
      rows="3"
      bind:value={message}
      readonly={pending}
      aria-describedby="tag-kind-{uid}"
    ></textarea>
    <p class="field-hint" id="tag-kind-{uid}">
      {annotated
        ? "Annotated: it records the message, who made it and when."
        : "Without a message the tag is lightweight: a name for the commit, nothing more."}
    </p>
  </div>
</Dialog>
