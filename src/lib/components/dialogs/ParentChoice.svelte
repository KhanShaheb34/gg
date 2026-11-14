<!--
  For a merge commit: which parent its changes are taken relative to (git's `-m`, 1-based),
  each parent shown by short hash and subject (from the loaded rows, else read from the core
  as the dialog opens). The
  first is chosen to begin with: it is the branch the merge was made on.
-->
<script lang="ts">
  import { onMount } from "svelte";
  import { api } from "../../ipc";
  import { findRepoView } from "../../state/repo.svelte";
  import CommitRef from "./CommitRef.svelte";

  type Props = {
    repoId: number;
    /** The merge's parents, full hashes, in order. */
    parents: string[];
    legend: string;
    /** The chosen parent, 1-based. */
    mainline: number;
  };

  let { repoId, parents, legend, mainline = $bindable() }: Props = $props();

  /** Subjects of the parents, by hash: from the loaded rows at once, the others as read. */
  let subjects: Record<string, string> = $state({});
  onMount(() => {
    const loaded = findRepoView(repoId)?.cache;
    for (const parent of parents) {
      const row = loaded?.byHash(parent);
      if (row) subjects[parent] = row.subject;
    }
  });
  const uid = Math.random().toString(36).slice(2, 8);

  onMount(() => {
    for (const parent of parents) {
      if (parent in subjects) continue;
      api
        .commitDetails(repoId, parent)
        .then((d) => (subjects[parent] = d.message.split("\n", 1)[0]))
        .catch(() => {}); // the short hash alone says which it is
    }
  });
</script>

<fieldset class="choices">
  <legend class="field-label">{legend}</legend>
  {#each parents as parent, i (parent)}
    <label class="choice">
      <input
        class="radio"
        type="radio"
        name="parent-{uid}"
        value={i + 1}
        bind:group={mainline}
        data-autofocus={i + 1 === mainline ? "" : undefined}
      />
      <span class="text">
        <CommitRef hash={parent} subject={subjects[parent] ?? null} />
        <span class="choice-text">
          Parent {i + 1}: {i === 0 ? "the branch it was merged into" : "the branch merged in"}
        </span>
      </span>
    </label>
  {/each}
</fieldset>

<style>
  .text {
    min-width: 0;
  }
</style>
