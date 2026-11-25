<!--
  A comment's body, drawn from the tree markdown.ts makes of it with this component's own
  elements only: no HTML of the comment reaches the DOM, nothing in it is fetched. Headings
  are drawn small (a comment is not a document); code scrolls sideways; tables scroll; task
  boxes are read-only; <details> are closed disclosures. A link is a link only for an https
  address: its full address in its tooltip, opened in the browser (`onlink`) on a click or
  Enter, never followed by the page itself (it has no href).
-->
<script lang="ts">
  import type { Block, Inline } from "../markdown";

  type Props = {
    blocks: Block[];
    /** Opens `url` (an https address) in the browser. */
    onlink: (url: string) => void;
  };

  let { blocks, onlink }: Props = $props();

  function activate(e: MouseEvent | KeyboardEvent, url: string) {
    if (e instanceof KeyboardEvent && e.key !== "Enter") return;
    e.preventDefault();
    e.stopPropagation();
    onlink(url);
  }
</script>

{#snippet inline(nodes: Inline[])}
  {#each nodes as n, i (i)}
    {#if n.k === "text"}{n.text}{:else if n.k === "br"}<br />{:else if n.k === "code"}<code class="md-code">{n.text}</code>{:else if n.k === "strong"}<strong>{@render inline(n.children)}</strong>{:else if n.k === "em"}<em>{@render inline(n.children)}</em>{:else if n.k === "del"}<del>{@render inline(n.children)}</del>{:else if n.k === "image"}<span class="md-image" title="An image, not loaded">[image: {n.alt}]</span>{:else if n.k === "link"}{#if n.url !== null}<span
          class="md-link"
          role="link"
          tabindex="0"
          title={n.url}
          data-url={n.url}
          onclick={(e) => activate(e, n.url!)}
          onkeydown={(e) => activate(e, n.url!)}>{@render inline(n.children)}</span
        >{:else}{@render inline(n.children)}{/if}{/if}
  {/each}
{/snippet}

{#snippet list(bs: Block[])}
  {#each bs as b, i (i)}
    {#if b.k === "p"}
      <p class="md-p">{@render inline(b.children)}</p>
    {:else if b.k === "plain"}
      <div class="md-plain">{@render inline(b.children)}</div>
    {:else if b.k === "heading"}
      <p class="md-h md-h{Math.min(b.level, 3)}">{@render inline(b.children)}</p>
    {:else if b.k === "code"}
      <pre class="md-pre" tabindex="-1"><code>{b.text}</code></pre>
    {:else if b.k === "list"}
      {#if b.ordered}
        <ol class="md-list" start={b.start}>
          {#each b.items as item, j (j)}<li>{@render list(item.blocks)}</li>{/each}
        </ol>
      {:else}
        <ul class="md-list" class:tasks={b.items.some((it) => it.task !== null)}>
          {#each b.items as item, j (j)}
            <li class:task={item.task !== null}>
              {#if item.task !== null}<input
                  class="checkbox md-task"
                  type="checkbox"
                  checked={item.task}
                  disabled
                  tabindex="-1"
                  aria-label={item.task ? "Done" : "Not done"}
                />{/if}
              <div class="md-item">{@render list(item.blocks)}</div>
            </li>
          {/each}
        </ul>
      {/if}
    {:else if b.k === "quote"}
      <blockquote class="md-quote" data-alert={b.alert?.toLowerCase()}>
        {#if b.alert}<p class="md-alert">{b.alert}</p>{/if}
        {@render list(b.blocks)}
      </blockquote>
    {:else if b.k === "hr"}
      <hr class="md-hr" />
    {:else if b.k === "table"}
      <div class="md-table">
        <table>
          <thead>
            <tr>
              {#each b.head as cell, j (j)}<th style:text-align={b.align[j] ?? undefined}>{@render inline(cell)}</th>{/each}
            </tr>
          </thead>
          <tbody>
            {#each b.rows as row, r (r)}
              <tr>
                {#each row as cell, j (j)}<td style:text-align={b.align[j] ?? undefined}>{@render inline(cell)}</td>{/each}
              </tr>
            {/each}
          </tbody>
        </table>
      </div>
    {:else if b.k === "details"}
      <details class="md-details">
        <summary>{@render inline(b.summary)}</summary>
        <div class="md-details-body">{@render list(b.blocks)}</div>
      </details>
    {/if}
  {/each}
{/snippet}

<div class="md">{@render list(blocks)}</div>

<style>
  .md {
    min-width: 0;
    color: var(--text);
    font-size: var(--fs-sm);
    line-height: 1.5;
    overflow-wrap: anywhere;
  }

  .md :global(> :first-child),
  .md :global(.md-details-body > :first-child),
  .md :global(.md-quote > :first-child),
  .md :global(.md-item > :first-child) {
    margin-top: 0;
  }

  .md :global(> :last-child),
  .md :global(.md-details-body > :last-child),
  .md :global(.md-quote > :last-child),
  .md :global(.md-item > :last-child) {
    margin-bottom: 0;
  }

  .md-p,
  .md-h,
  .md-pre,
  .md-list,
  .md-quote,
  .md-table,
  .md-details {
    margin: 0 0 8px;
  }

  /* Headings, small: a comment's sections, not a document's. */
  .md-h {
    margin-top: 10px;
    font-size: var(--fs-sm);
    font-weight: 650;
    line-height: 1.4;
  }

  .md-h1 {
    font-size: var(--fs);
  }

  .md-h3 {
    color: var(--text-muted);
  }

  .md-code {
    padding: 0.5px 4px;
    border: 1px solid var(--border);
    border-radius: 4px;
    background: var(--bg-chrome);
    font-family: var(--font-mono);
    font-size: 0.92em;
    overflow-wrap: anywhere;
  }

  .md-pre {
    max-width: 100%;
    padding: 7px 9px;
    overflow-x: auto;
    border: 1px solid var(--border);
    border-radius: var(--radius);
    background: var(--bg-chrome);
    font-family: var(--font-mono);
    font-size: var(--fs-xs);
    line-height: 1.5;
    white-space: pre;
    overflow-wrap: normal;
    user-select: text;
    -webkit-user-select: text;
  }

  .md-pre:focus-visible {
    outline-offset: -2px;
  }

  .md-list {
    padding-left: 18px;
  }

  .md-list li + li {
    margin-top: 2px;
  }

  .md-list.tasks {
    padding-left: 0;
    list-style: none;
  }

  .md-list li.task {
    display: flex;
    align-items: baseline;
    gap: 6px;
  }

  .md-task {
    position: relative;
    top: 2px;
    width: 12px;
    height: 12px;
    border-radius: 3px;
  }

  .md-item {
    flex: 1 1 auto;
    min-width: 0;
  }

  .md-quote {
    padding: 1px 0 1px 10px;
    border-left: 2px solid var(--border-strong);
    color: var(--text-muted);
  }

  .md-quote[data-alert] {
    --alert: var(--info);
    border-left-color: color-mix(in srgb, var(--alert) 70%, transparent);
  }

  .md-quote[data-alert="warning"],
  .md-quote[data-alert="important"] {
    --alert: var(--warning);
  }

  .md-quote[data-alert="caution"] {
    --alert: var(--danger);
  }

  .md-quote[data-alert="tip"] {
    --alert: var(--success);
  }

  .md-alert {
    margin-bottom: 4px;
    color: color-mix(in srgb, var(--alert) 85%, var(--text));
    font-size: var(--fs-xs);
    font-weight: 650;
  }

  .md-hr {
    height: 1px;
    margin: 10px 0;
    border: 0;
    background: var(--border);
  }

  .md-table {
    max-width: 100%;
    overflow-x: auto;
  }

  .md-table table {
    border-collapse: collapse;
    font-size: var(--fs-xs);
  }

  .md-table th,
  .md-table td {
    padding: 4px 8px;
    border: 1px solid var(--border);
    text-align: left;
    vertical-align: top;
    overflow-wrap: normal;
    min-width: 72px;
    max-width: 320px;
  }

  .md-table th {
    background: var(--bg-chrome);
    font-weight: 600;
  }

  .md-details > summary {
    display: flex;
    align-items: baseline;
    gap: 5px;
    width: fit-content;
    max-width: 100%;
    padding: 1px 4px 1px 0;
    border-radius: 4px;
    color: var(--text-muted);
    font-weight: 500;
    list-style: none;
    cursor: default;
  }

  .md-details > summary::-webkit-details-marker {
    display: none;
  }

  /* A small chevron, turning down when open. */
  .md-details > summary::before {
    content: "";
    flex: none;
    width: 5px;
    height: 5px;
    margin: 0 2px 0 1px;
    border-right: 1.3px solid var(--text-faint);
    border-bottom: 1.3px solid var(--text-faint);
    transform: translateY(-1px) rotate(-45deg);
    transition: transform 80ms var(--ease);
  }

  .md-details[open] > summary::before {
    transform: translateY(-2px) rotate(45deg);
  }

  .md-details > summary:hover {
    color: var(--text);
  }

  .md-details > summary:focus-visible {
    outline-offset: 1px;
  }

  .md-details-body {
    margin: 6px 0 0 3px;
    padding-left: 10px;
    border-left: 1px solid var(--border);
  }

  .md-link {
    color: var(--accent);
    text-decoration: underline;
    text-decoration-color: color-mix(in srgb, var(--accent) 40%, transparent);
    text-underline-offset: 2px;
    cursor: pointer;
  }

  .md-link:hover {
    text-decoration-color: currentColor;
  }

  .md-link:focus-visible {
    border-radius: 2px;
    outline-offset: 1px;
  }

  .md-image {
    color: var(--text-faint);
    font-size: var(--fs-xs);
  }

  del {
    color: var(--text-muted);
  }
</style>
