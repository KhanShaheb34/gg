<!--
  The commit a dialog acts on: its short hash and subject, on one line (the subject cut with
  an ellipsis; the whole of it in the tooltip). `subject` null: not known yet.
-->
<script lang="ts">
  import { shortHash, visibleText } from "../../format";

  let { hash, subject }: { hash: string; subject: string | null } = $props();
</script>

<span class="commit-ref" title={subject ?? undefined}>
  <span class="hash">{shortHash(hash)}</span>
  <bdi class="subject">{subject === null ? "" : visibleText(subject)}</bdi>
</span>

<style>
  .commit-ref {
    display: flex;
    align-items: baseline;
    gap: 8px;
    min-width: 0;
  }

  .hash {
    flex: none;
    color: var(--text-muted);
    font-family: var(--font-mono);
    font-size: var(--fs-sm);
  }

  .subject {
    min-width: 0;
    overflow: hidden;
    white-space: nowrap;
    text-overflow: ellipsis;
  }
</style>
